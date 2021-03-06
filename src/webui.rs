use super::audioplumbing;
use rocket::{
    response::{content, NamedFile},
    State,
};
use std::path::{Path, PathBuf};

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
pub fn stats(controller: State<audioplumbing::Controller<f32>>) -> String {
    match controller.get_frame_stats() {
        Ok(fs) => fs.format_stats(),
        Err(why) => format!("error getting frame stats: {:?}", why),
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
