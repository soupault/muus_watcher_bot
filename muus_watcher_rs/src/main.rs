mod muus;

use std::sync::Arc;
use std::path::Path;
use std::collections::{HashSet, HashMap};
use chrono::prelude::*;
use nanodb::nanodb::NanoDB;
use tokio;
use tokio::{sync::Mutex, time};
use frankenstein::{AsyncTelegramApi, ChatId, Message, AsyncApi, UpdateContent,
                   GetUpdatesParams, ReplyParameters, SendMessageParams};
use frankenstein::ParseMode::Html;
use serde::{Deserialize, Serialize};
use crate::muus::{MuusSearch, search_muus_market};


const TG_BOT_TOKEN: &str = "";  // ACT: set with the bot token obtained from @BotFather

const TG_POLL_INTERVAL: u64 = 5;  // in seconds, how freq-ly to poll user input from the bot
const QUERY_MONITOR_INTERVAL: u64 = 60;  // in seconds, how freq-ly to monitor the query database
const QUERY_COOLDOWN_INTERVAL: u64 = 3600;  // in seconds, period between same query executions
const PATH_DB: &str = "./db.json";


#[derive(Clone)]
struct SharedState {
    db: NanoDB,
}

#[derive(Debug)]
enum TgCommand {
    Help,
    Start,
    Add(String),
    List,
    Remove(Vec<u64>),
    Clear,
    Stop,
    Unknown(String),
}

#[derive(Serialize, Deserialize)]
struct DBUser {
    chat_id: i64,
    user_id: String,
    queries: Vec<DBQuery>,
}

#[derive(Serialize, Deserialize, Debug)]
struct DBQuery {
    index: u64,
    text: String,
    dtime_prev_req: DateTime<Utc>,
}


#[tokio::main]
async fn main() {
    let db;
    if Path::new(PATH_DB).exists() {
        db = NanoDB::open(PATH_DB).unwrap();
    } else {
        db = NanoDB::new_from(PATH_DB, r#"{}"#).unwrap();
    }

    let state = SharedState { db };
    let state_shared = Arc::new(Mutex::new(state));
    let state_search = Arc::clone(&state_shared);
    let state_repl = Arc::clone(&state_shared);

    let api_shared = Arc::new(AsyncApi::new(TG_BOT_TOKEN));
    let update_params_builder = GetUpdatesParams::builder();
    let mut update_params = update_params_builder.clone().build();
    let api_search = Arc::clone(&api_shared);
    let api_repl = Arc::clone(&api_shared);

    // Muus search
    let handle_search = tokio::spawn(async move {
        worker_search(state_search.clone(), api_search.clone()).await;
    });

    // REPL handling
    let handle_repl = tokio::spawn(async move {
        loop {
            let ret = api_repl.clone().get_updates(&update_params).await;
            match ret {
                Ok(response) => {
                    for update in response.result {
                        if let UpdateContent::Message(message) = update.content {
                            task_repl(state_repl.clone(), api_repl.clone(), message.clone()).await;
                        }
                        update_params = update_params_builder
                            .clone()
                            .offset(update.update_id + 1)
                            .build();
                    }
                }
                Err(error) => { println!("Failed to get updates: {error:?}") }
            }
            time::sleep(time::Duration::from_secs(TG_POLL_INTERVAL)).await;
        }
    });

    let _ = tokio::join!(handle_search, handle_repl);
}

async fn worker_search(state: Arc<Mutex<SharedState>>, api: Arc<AsyncApi>) {
    let mut interval_mon = time::interval(time::Duration::from_secs(QUERY_MONITOR_INTERVAL));
    let mut interval_reqs = time::interval(time::Duration::from_secs(1));

    loop {
        let mut state = state.lock().await;
        let mut mss_todo: Vec<MuusSearch> = Vec::new();

        // Build a vector of searches to run
        let dtime_now = Utc::now();
        let t = state.db.data().await.into::<HashMap<String,DBUser>>().unwrap();
        for (_, db_user) in t.iter() {
            let db_qs = &db_user.queries;
            for q in db_qs.iter() {
                // println!("Chat ID {}, query index {}, text {}", chat_id_str.clone(), q.text, q.index);
                if (dtime_now - q.dtime_prev_req).num_seconds() >= QUERY_COOLDOWN_INTERVAL as i64 {
                    let ms = MuusSearch {
                        index: q.index, text: q.text.clone(), user_id: db_user.user_id.clone(),
                        chat_id: db_user.chat_id.clone(), dtime_req: dtime_now.clone()
                    };
                    mss_todo.push(ms);
                }
            }
        }

        for ms_todo in mss_todo.iter() {
            // println!("Requesting {} {}", ms_todo.user_id, ms_todo.text.clone());
            let ret = search_muus_market(ms_todo.clone()).await;
            let chat_id_str = format!("{}", ms_todo.chat_id);
            match ret {
                Ok(listings) => {
                    // Update the query time in the db
                    let qs = state.db.update().await.get(&chat_id_str).unwrap()
                        .get("queries").unwrap().into::<Vec<DBQuery>>().unwrap();
                    let q_pos = qs.iter().position(|x| x.index.eq(&ms_todo.index)).unwrap();
                    let q_upd = DBQuery {
                        index: ms_todo.index,
                        text: ms_todo.text.clone(),
                        dtime_prev_req: ms_todo.dtime_req,
                    };
                    _ = state.db.update().await.get(&chat_id_str).unwrap()
                        .get("queries").unwrap().remove_at(q_pos);
                    _ = state.db.update().await.get(&chat_id_str).unwrap()
                        .get("queries").unwrap().push(q_upd);
                    state.db.write().await.ok();

                    for listing in listings {
                        // Compose the message
                        let mut response = format!("<i>From query '{}':</i>", ms_todo.text.clone());
                        response.push_str(format!("\n<b>{}</b>", listing.title).as_str());
                        response.push_str(format!("\n{}", listing.url).as_str());
                        response.push_str(format!("\n{}", listing.dtime_upd).as_str());

                        // Send news listings to the chat
                        let smp = SendMessageParams::builder()
                            .chat_id(ChatId::Integer(ms_todo.chat_id))
                            .text(response)
                            .parse_mode(Html)
                            .build();

                        if let Err(err) = api.send_message(&smp).await {
                            println!("Failed to send message: {err:?}");
                        }
                    }
                },
                Err(_) => println!("Query for user {} failed: {}", ms_todo.user_id, ms_todo.text),
            };
            // Wait random small interval
            interval_reqs.tick().await;
        }
        interval_mon.tick().await;
    }
}

fn parse_command(text: &str) -> TgCommand {
    let mut parts = text.trim().split_whitespace();
    let command = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();

    match command {
        "/help" => TgCommand::Help,
        "/start" => TgCommand::Start,
        "/add" => TgCommand::Add(args.join("+")),
        "/list" => TgCommand::List,
        "/remove" => TgCommand::Remove(args.iter().map(|e| e.parse::<u64>().unwrap()).collect()),
        "/clear" => TgCommand::Clear,
        "/stop" => TgCommand::Stop,
        _ => TgCommand::Unknown(text.to_string()),
    }
}

async fn task_repl(state: Arc<Mutex<SharedState>>, api: Arc<AsyncApi>, message: Message) {
    let mut state = state.lock().await;
    let user_id = message.chat.username.unwrap();
    let chat_id = message.chat.id;
    let chat_id_str = format!("{}", message.chat.id);
    let chat_id_known = state.db.read().await.get(&chat_id_str).is_ok();

    let reply_parameters = ReplyParameters::builder()
        .message_id(message.message_id)
        .build();

    let mut response = String::new();
    match parse_command(&message.text.unwrap()) {
        TgCommand::Help => { response = "8==(D".to_string() },
        TgCommand::Start => {
            if chat_id_known {
                response = format!("\u{1F64B} You already have an account under ID <b>{}</b>.", user_id);
            } else {
                let entry = DBUser {
                    chat_id: chat_id.clone(),
                    user_id: user_id.clone(),
                    queries: Vec::<DBQuery>::new(),
                };
                state.db.update().await.insert(&chat_id_str, entry).ok();
                state.db.write().await.ok();
                response = format!("\u{1F64B} Welcome to the service! Your account ID is <b>{}</b> .", user_id);
            }
        },
        TgCommand::Add(q_new) => {
            if chat_id_known {
                if q_new.is_empty() {
                    response = "\u{02757} Cannot create an empty query.".to_string();
                } else {
                    let qs: Vec<DBQuery> = state.db.read().await.get(&chat_id_str).unwrap()
                        .get("queries").unwrap().into().unwrap();
                    let qs_texts: Vec<String> = qs.iter().map(|x| x.text.clone()).collect();
                    if qs_texts.contains(&q_new) {
                        response = format!("\u{02757} Query already exists: <b>{}</b>.", q_new);
                    } else {
                        // Use the first available index
                        let qs_idcs: HashSet<u64> = qs.iter().map(|x| x.index.clone()).collect();
                        let seq_idcs = HashSet::from_iter(0..(qs.len() as u64) + 1);
                        let idx = *seq_idcs.difference(&qs_idcs).next().unwrap();
                        let dbq = DBQuery {
                            index: idx,
                            text: q_new.clone(),
                            dtime_prev_req: Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap()
                        };
                        _ = state.db.update().await.get(&chat_id_str).unwrap()
                            .get("queries").unwrap().push(dbq);
                        state.db.write().await.ok();
                        response = format!("\u{02705} Query added: <b>{}</b>.", q_new);
                    }
                }
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::List => {
            if chat_id_known {
                let qs: Vec<DBQuery> = state.db.read().await.get(&chat_id_str).unwrap()
                    .get("queries").unwrap().into().unwrap();
                if qs.len() > 0 {
                    response = "\u{1F4CB} Existing queries:".to_string();
                    for q in qs.iter() {
                        response.push_str(format!("\n[{}]: <b>{}</b> (upd: {})",
                                                  q.index, q.text, q.dtime_prev_req).as_str());
                    }
                } else { response = "\u{1F4CB} No queries found.".to_string(); }
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Remove(idcs) => {
            if chat_id_known {
                let qs: Vec<DBQuery> = state.db.read().await.get(&chat_id_str).unwrap()
                    .get("queries").unwrap().into::<Vec<DBQuery>>().unwrap();
                for idx in idcs.iter() {
                    match qs.iter().position(|x| x.index.eq(idx)) {
                        Some(q_pos) => {
                            state.db.update().await.get(&chat_id_str).unwrap()
                                .get("queries").unwrap().remove_at(q_pos).ok();
                            state.db.write().await.ok();
                            response.push_str(format!("\u{0274E} Query removed: <b>{}</b> (<b>{}</b>).\n",
                                                      idx, qs[q_pos].text).as_str());
                        }
                        None => { response.push_str(format!("\u{02757} Incorrect index: <b>{}</b>.\n", idx).as_str()); }
                    }
                }
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Clear => {
            if chat_id_known {
                let v: Vec<DBQuery> = Vec::new();
                _ = state.db.update().await.get(&chat_id_str).unwrap().insert("queries", v);
                state.db.write().await.ok();
                response = "\u{1F6BD} All queries cleared.".to_string();
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Stop => {
            if chat_id_known {
                state.db.update().await.remove(&chat_id_str).ok();
                state.db.write().await.ok();
                response = "\u{1F64B} Your account has been removed.".to_string();
            } else { response = "\u{1F4CB} There is no account associated with you.".to_string(); }
        },
        TgCommand::Unknown(_) => { response = "Unknown command.".to_string(); },
    }

    if !response.is_empty() {
        let smp = SendMessageParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .text(response)
            .parse_mode(Html)
            .reply_parameters(reply_parameters)
            .build();
        if let Err(err) = api.send_message(&smp).await {
            println!("Failed to send message: {err:?}");
        }
    }
}
