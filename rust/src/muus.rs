use std::fmt;
use select::document::Document;
use select::predicate::Attr;
use regex::Regex;
use chrono::prelude::*;


#[derive(Clone)]
pub(crate) struct MuusSearch {
    pub index: u64,
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

pub(crate) async fn search_muus_market(search: MuusSearch) -> Result<Vec<MuusListing>, Box<dyn std::error::Error + Send>> {
    let query = [("keyword", search.text)];
    let client = reqwest::Client::new();
    let resp: String = client.post("https://muusikoiden.net/tori/haku.php")
        .query(&query)
        .send()
        .await.unwrap()  // TODO: should handle potential errors here better
        .text()
        .await.unwrap();
    // println!("{resp:?}");

    let document = Document::from(resp.as_str());

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
            println!("{}", listing);
            listings.push(listing);
        }
    }

    return Ok(listings)
}
