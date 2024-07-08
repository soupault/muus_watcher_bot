use std::fmt;
use select::document::Document;
use select::predicate::Attr;
use regex::Regex;
use chrono::prelude::*;
use reqwest;
use serde;

#[derive(Clone)]
pub(crate) struct MuusSearch {
    pub uniq_id: u64,
    pub text: String,
    pub user_id: String,
    pub chat_id: i64,
    pub dtime_req: DateTime<Utc>,
}

pub(crate) struct MuusListing {
    pub title: String,
    pub url: String,
    pub dtime_add: DateTime<Utc>,
    pub dtime_upd: DateTime<Utc>,
}

impl fmt::Display for MuusListing {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} | {} | {} | {}", self.title, self.url,
               self.dtime_add.format("%Y-%m-%d %H:%M:%S").to_string(),
               self.dtime_add.format("%Y-%m-%d %H:%M:%S").to_string())
    }
}

fn page_to_listings(document: Document) -> Vec<MuusListing> {
    let mut listings: Vec<MuusListing> = Vec::new();

    for node in document.find(Attr("class", "bg2")) {
        for title in node.find(Attr("class", "tori_title")) {
            let l_title = title.find(Attr("href", ())).next().unwrap().text();

            let t = title.find(Attr("href", ())).next().unwrap().html();
            let re = Regex::new(r#"href="([^"]+)""#).unwrap();
            let p = re.captures(&t).unwrap().get(1).unwrap().as_str();  // 0th is the matched string
            let l_url = format!("https://muusikoiden.net{p}");

            let t = node.find(Attr("title", ())).next().unwrap().html();
            let re = Regex::new(r#"Lisätty: (\d{2}).(\d{2}).(\d{4}) (\d{2}):(\d{2})"#).unwrap();
            let mut l_dtime_add = Utc::now();
            for (_, ret) in re.captures_iter(&t).map(|c| c.extract()).take(1) {
                let [d, mo, y, h, mi] = ret.map(|e| e.parse::<u32>().unwrap());
                l_dtime_add = Utc.with_ymd_and_hms(y as i32, mo, d, h, mi, 0).unwrap();
            }

            let re = Regex::new(r#"Muokattu: (\d{2}).(\d{2}).(\d{4}) (\d{2}):(\d{2})"#).unwrap();
            let mut l_dtime_upd = Utc::now();
            for (_, ret) in re.captures_iter(&t).map(|c| c.extract()).take(1) {
                let [d, mo, y, h, mi] = ret.map(|e| e.parse::<u32>().unwrap());
                l_dtime_upd = Utc.with_ymd_and_hms(y as i32, mo, d, h, mi, 0).unwrap();
            }
            // println!();
            // println!("Found: {} {}", node.html(), t);
            // println!("title {}", l_title);
            // println!("url {}", l_url);
            // println!("dtime_add {}", l_dtime_add.format("%Y-%m-%d %H:%M:%S").to_string());
            // println!("dtime_upd {}", l_dtime_upd.format("%Y-%m-%d %H:%M:%S").to_string());
            // println!();

            let listing = MuusListing {
                title: l_title,
                url: l_url,
                dtime_add: l_dtime_add,
                dtime_upd: l_dtime_upd,
            };
            // println!("{}", listing);
            listings.push(listing);
        }
    }
    return listings;
}

fn search_muus_market() {
    let mut listings: Vec<MuusListing> = Vec::new();

    // First page of the results
    let client = reqwest::blocking::Client::new();
    let query_init = [("keyword", "elektron".to_string())];
    let resp: String = client.post("https://muusikoiden.net/tori/haku.php")
        .query(&query_init).send().unwrap()  // TODO: should handle potential errors here better
        .text().unwrap();

    let mut resp_str = resp.to_string();
    let mut page = Document::from(resp_str.as_str());
    listings.extend(page_to_listings(page.clone()).into_iter());

    // Additional pages
    loop {
        let re = Regex::new(r#"<a href="([^"]+)">seuraava</a>"#).unwrap();
        if let Some(p) = re.captures(&resp_str) {
            let matched = p.get(1).unwrap().as_str();
            // println!("Matched: {}", matched.clone());

            let mut query_cont: Vec<(&str, String)> = Vec::new();
            let params = matched.split("?").nth(1).unwrap();
            for param in params.split("&amp;") {
                if let Some(pos) = param.find("=") {
                    let k = &param[..pos];
                    let v = param[pos + 1..].to_string();
                    if k != "checksum" {
                        query_cont.push((k, v));
                    }
                }
            }
            // println!("Parsed:");
            // for (k, v) in &query_cont { println!("{} {}", k.to_string(), v); }

            let resp: String = client.post("https://muusikoiden.net/tori/haku.php")
                .query(&query_cont).send().unwrap()
                .text().unwrap();
            // println!("{resp:?}");
            resp_str = resp.to_string();
            page = Document::from(resp_str.as_str());
            listings.extend(page_to_listings(page.clone()).into_iter());
        } else {
            break
        };
    }

    listings = listings.into_iter().rev().collect();  // Sort in chronological order
    // println!("Listings found: {}", listings.iter().count());
}

fn main() {
    search_muus_market();
    println!("Hello, world!");
}