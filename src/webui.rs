use super::audioplumbing;
use rocket::{
    response::{content, NamedFile},
    State,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, SyncSender};

#[get("/")]
pub fn index() -> content::Html<String> {
    content::Html(format!(
        "Hello, world!\
            <ul>\
            <li><a href=\"dart/index.html\">dart index.html</a></li>\
            <li><a href=\"stats\">stats</a>\
            </ul>"
    ))
}

#[get("/stats")]
pub fn stats(state: State<SyncSender<audioplumbing::Request<f32>>>) -> String {
    let (resp_sender, resp_receiver) = channel();
    state
        .send(audioplumbing::Request::StatsRequest {
            resp: resp_sender.clone(),
        })
        .unwrap();
    match resp_receiver.recv() {
        Ok(fs) => fs.format_stats(),
        Err(why) => format!("error receiving stats: {:?}", why),
    }
}

#[get("/hello_world")] // <- route attribute
pub fn hi_world() -> &'static str {
    "hello, world!"
}

#[get("/dart/<file..>")]
pub fn dart_files(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("../dart/build/").join(file)).ok()
}
