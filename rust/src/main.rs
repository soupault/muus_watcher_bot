mod muus;

use core::time::Duration;
use std::{fs, thread};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::path::Path;
use chrono::prelude::*;
use frankenstein::{ChatId, Message, Api, UpdateContent, GetUpdatesParams, ReplyParameters,
                   SendMessageParams, TelegramApi};
use frankenstein::ParseMode::Html;
use serde::{Deserialize, Serialize};
use serde_json;
use crate::muus::{MuusSearch, search_muus_market};


const TG_BOT_TOKEN: &str = "";  // ACT: set with the bot token obtained from @BotFather

const TG_POLL_INTERVAL: u64 = 5;  // in seconds, how freq-ly to poll user input from the bot
const QUERY_MONITOR_INTERVAL: u64 = 60;  // in seconds, how freq-ly to monitor the query database
const QUERY_COOLDOWN_INTERVAL: u64 = 3600;  // in seconds, period between same query executions
const PATH_DB: &str = "./db.json";


#[derive(Debug)]
enum TgCommand {
    Help,
    Start,
    Add(String),
    List,
    Remove(Vec<u64>),
    Clear,
    Stop,
    Unknown(()),
}

type DBRoot = Vec<DBUser>;

#[derive(Serialize, Deserialize, Debug)]
struct DBUser {
    chat_id: i64,
    user_id: String,
    queries: Vec<DBQuery>,
}

#[derive(Serialize, Deserialize, Debug)]
struct DBQuery {
    uniq_id: u64,
    text: String,
    dtime_prev_req: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
struct SharedState {
    db: DBRoot,
}

fn main() {
    let db: Vec<DBUser>;
    if Path::new(PATH_DB).exists() {
        let fc = fs::read_to_string(PATH_DB).unwrap();
        db = serde_json::from_str(&fc).unwrap();
    } else {
        db = serde_json::from_str(r#"[]"#).unwrap();
    }

    let state = SharedState { db };
    let state_shared = Arc::new(Mutex::new(state));
    let state_search = Arc::clone(&state_shared);
    let state_repl = Arc::clone(&state_shared);

    let api_shared = Arc::new(Api::new(TG_BOT_TOKEN));
    let update_params_builder = GetUpdatesParams::builder();
    let mut update_params = update_params_builder.clone().build();
    let api_search = Arc::clone(&api_shared);
    let api_repl = Arc::clone(&api_shared);

    // Muus search
    let handle_search = thread::spawn(move || {
        task_search(state_search.clone(), api_search.clone())
    });

    // REPL handling
    let handle_repl = thread::spawn(move || {
        loop {
            let ret = api_repl.clone().get_updates(&update_params);
            match ret {
                Ok(response) => {
                    for update in response.result {
                        if let UpdateContent::Message(message) = update.content {
                            task_repl(state_repl.clone(), api_repl.clone(), message.clone());
                        }
                        update_params = update_params_builder
                            .clone()
                            .offset(update.update_id + 1)
                            .build();
                    }
                }
                Err(error) => { println!("Failed to get updates: {error:?}") }
            }
            thread::sleep(Duration::from_secs(TG_POLL_INTERVAL));
        }
    });

    _ = handle_search.join();
    _ = handle_repl.join();
}

fn task_search(state: Arc<Mutex<SharedState>>, api: Arc<Api>) {
    loop {
        let mut state = state.lock().unwrap();
        let mut mss_todo: Vec<MuusSearch> = Vec::new();

        // Build a vector of searches to run
        let dtime_now = Utc::now();
        for db_user in state.db.iter() {
            let db_qs = &db_user.queries;
            for q in db_qs.iter() {
                // println!("Chat ID {}, query index {}, text {}", chat_id_str.clone(), q.text, q.index);
                if (dtime_now - q.dtime_prev_req).num_seconds() >= QUERY_COOLDOWN_INTERVAL as i64 {
                    let ms = MuusSearch {
                        user_id: db_user.user_id.clone(),
                        chat_id: db_user.chat_id.clone(),
                        uniq_id: q.uniq_id.clone(),
                        text: q.text.clone(),
                        dtime_req: dtime_now.clone()
                    };
                    mss_todo.push(ms);
                }
            }
        }

        for ms_todo in mss_todo.iter() {
            // println!("Requesting {} {}", ms_todo.user_id, ms_todo.text.clone());
            let ret = search_muus_market(ms_todo.clone());
            let pos_user = state.db.iter().position(|u| u.chat_id == ms_todo.chat_id).unwrap();
            match ret {
                Ok(listings) => {
                    // Update the query time in the db
                    let qs = &state.db.get(pos_user).unwrap().queries;
                    let q_pos = qs.iter().position(|q| q.uniq_id.eq(&ms_todo.uniq_id)).unwrap();
                    let q_upd = DBQuery {
                        uniq_id: ms_todo.uniq_id.clone(),
                        text: ms_todo.text.clone(),
                        dtime_prev_req: ms_todo.dtime_req,
                    };
                    _ = state.db.get_mut(pos_user).unwrap().queries.remove(q_pos);
                    _ = state.db.get_mut(pos_user).unwrap().queries.push(q_upd);
                    _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());

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

                        if let Err(err) = api.send_message(&smp) {
                            println!("Failed to send message: {err:?}");
                        }
                    }
                },
                Err(_) => println!("Query for user {} failed: {}", ms_todo.user_id, ms_todo.text),
            };
            // Wait random small interval
            thread::sleep(Duration::from_secs(1));
        }
        drop(state);
        thread::sleep(Duration::from_secs(QUERY_MONITOR_INTERVAL));
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
        _ => TgCommand::Unknown(()),
    }
}

fn task_repl(state: Arc<Mutex<SharedState>>, api: Arc<Api>, message: Message) {
    let mut state = state.lock().unwrap();
    let user_id = message.chat.username.unwrap();
    let chat_id = message.chat.id;
    let chat_id_known = state.db.iter().map(|u| u.chat_id == chat_id).count() != 0usize;

    let reply_parameters = ReplyParameters::builder()
        .message_id(message.message_id)
        .build();

    let mut response = String::new();
    match parse_command(&message.text.unwrap()) {
        TgCommand::Help => { response = "See the supported commands in the menu.".to_string() },
        TgCommand::Start => {
            if chat_id_known {
                response = format!("\u{1F64B} You already have an account under ID <b>{}</b>.", user_id);
            } else {
                let qs: Vec<DBQuery> = Vec::new();
                let entry = DBUser {
                    chat_id: chat_id.clone(),
                    user_id: user_id.clone(),
                    queries: qs,
                };
                state.db.push(entry);
                _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());
                response = format!("\u{1F64B} Welcome to the service! Your account ID is <b>{}</b> .", user_id);
            }
        },
        TgCommand::Add(q_new) => {
            if chat_id_known {
                if q_new.is_empty() {
                    response = "\u{02757} Cannot create an empty query.".to_string();
                } else {
                    let pos_user = state.db.iter().position(|u| u.chat_id == chat_id).unwrap();
                    let qs: &Vec<DBQuery> = &state.db.get(pos_user).unwrap().queries;
                    if qs.iter().any(|q| q.text.eq(&q_new)) {
                        response = format!("\u{02757} Query already exists: <b>{}</b>.", q_new);
                    } else {
                        // Use the first available index
                        let qs_ids: HashSet<u64> = qs.iter().map(|x| x.uniq_id.clone()).collect();
                        let seq_idcs = HashSet::from_iter(0..(qs.len() as u64) + 1);
                        let idx = *seq_idcs.difference(&qs_ids).next().unwrap();
                        let dbq = DBQuery {
                            uniq_id: idx,
                            text: q_new.clone(),
                            dtime_prev_req: Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap()
                        };
                        _ = state.db.get_mut(pos_user).unwrap().queries.push(dbq);
                        _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());
                        response = format!("\u{02705} Query added: <b>{}</b>.", q_new);
                    }
                }
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::List => {
            if chat_id_known {
                let pos_user = state.db.iter().position(|u| u.chat_id == chat_id).unwrap();
                let qs: &Vec<DBQuery> = &state.db.get(pos_user).unwrap().queries;
                if qs.len() > 0 {
                    response = "\u{1F4CB} Existing queries:".to_string();
                    for q in qs.iter() {
                        response.push_str(format!("\n[{}]: <b>{}</b> (upd: {})",
                                                  q.uniq_id, q.text, q.dtime_prev_req).as_str());
                    }
                } else { response = "\u{1F4CB} No queries found.".to_string(); }
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Remove(idcs) => {
            if chat_id_known {
                let pos_user = state.db.iter().position(|u| u.chat_id == chat_id).unwrap();
                for idx in idcs.iter() {
                    let qs_ids: Vec<u64> = state.db.get(pos_user).unwrap().queries
                        .iter().map(|q| q.uniq_id).collect();
                    let qs_texts: Vec<String> = state.db.get(pos_user).unwrap().queries
                        .iter().map(|q| q.text.clone()).collect();

                    match qs_ids.iter().position(|i| i.eq(idx)) {
                        Some(pos_q) => {
                            state.db.get_mut(pos_user).unwrap().queries.remove(pos_q);
                            response.push_str(format!("\u{0274E} Query removed: <b>{}</b> (<b>{}</b>).\n",
                                                      idx, qs_texts[pos_q]).as_str());
                        }
                        None => { response.push_str(format!("\u{02757} Incorrect index: <b>{}</b>.\n", idx).as_str()); }
                    }
                }
                _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Clear => {
            if chat_id_known {
                let pos_user = state.db.iter().position(|u| u.chat_id == chat_id).unwrap();
                let v: Vec<DBQuery> = Vec::new();
                _ = state.db.get_mut(pos_user).unwrap().queries = v;
                _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());
                response = "\u{1F6BD} All queries cleared.".to_string();
            } else { response = "\u{1F4CB} Need to create an account first.".to_string(); }
        },
        TgCommand::Stop => {
            if chat_id_known {
                let pos = state.db.iter().position(|u| u.chat_id == chat_id).unwrap();
                state.db.remove(pos);
                _ = fs::write(PATH_DB, serde_json::to_string_pretty(&state.db).unwrap());
                response = "\u{1F64B} Your account has been removed.".to_string();
            } else { response = "\u{1F4CB} There is no account associated with you.".to_string(); }
        },
        TgCommand::Unknown(_) => { response = "Unknown command.".to_string(); },
        // _ => { response = "Other command.".to_string(); },
    }

    if !response.is_empty() {
        let smp = SendMessageParams::builder()
            .chat_id(ChatId::Integer(chat_id))
            .text(response)
            .parse_mode(Html)
            .reply_parameters(reply_parameters)
            .build();
        if let Err(err) = api.send_message(&smp) {
            println!("Failed to send message: {err:?}");
        }
    }
}
