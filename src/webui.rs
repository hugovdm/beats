use super::audioplumbing;
use bson::{Bson, Document};
use json;
use rocket::{
    response::{content, NamedFile},
    State,
};
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[get("/metrics")]
pub fn metrics() -> String {
    let mut buffer = Vec::new();
    let encoder = prometheus::TextEncoder::new();

    // Gather the metrics.
    let metric_families = prometheus::gather();

    // Encode them to send.
    // use prometheus::{self, Encoder, TextEncoder};
    use prometheus::Encoder;
    encoder.encode(&metric_families, &mut buffer).unwrap();

    String::from_utf8(buffer.clone()).unwrap()
}

#[get("/")]
pub fn index() -> content::Html<String> {
    content::Html(format!(
        "Hello, world!\
        <ul>\
          <li><a href=\"dart/index.html\">dart index.html</a></li>\
          <li><a href=\"metrics\">metrics - for prometheus monitoring</a>\
          <li><a href=\"stats\">stats - prints info from last FrameStats</a>\
          <li><a href=\"status\">status - prints last chunk's envelope</a>\
          <li><a href=\"bson\">bson - returns last chunk's envelope in bson format</a>\
          <li><a href=\"json/last_chunk_envelope\">
            json/last_chunk_envelope - returns last chunk's envelope in json format</a>\
          <li><a href=\"json/envelope\">
            json/envelope - returns envelope of last several seconds in json format</a>\
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

#[get("/status")]
pub fn status(controller: State<audioplumbing::Controller<f32>>) -> String {
    match controller.get_status() {
        Ok(status_info) => {
            let mut s = String::new();
            use audioplumbing::ChunkTrait;
            for env_val in status_info.last_chunk.get_ro_envelope().iter() {
                writeln!(s, "{}", env_val).unwrap();
                let scalefactor: i16 = 256; // FIXME
                let e: i16 = cpal::Sample::to_i16(env_val) / scalefactor;
                for _ in 0..e {
                    s.push('#');
                }
                s.push('\n');
            }
            format!(
                "{}\n- {:?} saved blocks\n{} channels\nFIXME JSON, BSON, BJSON?",
                s,
                status_info.saved_blocks,
                status_info.last_chunk.get_channels()
            )
        }
        Err(why) => format!("error getting frame stats: {:?}", why),
    }
}

#[get("/bson")]
pub fn bson_env_chunk(controller: State<audioplumbing::Controller<f32>>) -> Vec<u8> {
    let mut buf = Vec::new();
    match controller.get_status() {
        Ok(status_info) => {
            use audioplumbing::ChunkTrait;
            let env = Bson::from(status_info.last_chunk.get_ro_envelope());
            let mut doc = Document::new();
            doc.insert("envelope", env);
            doc.to_writer(&mut buf).unwrap();
        }
        Err(why) => println!("error getting status in fn bson(): {:?}", why),
    }
    return buf;
}

#[get("/json/last_chunk_envelope")]
pub fn json_env_chunk(controller: State<audioplumbing::Controller<f32>>) -> String {
    match controller.get_status() {
        Ok(status_info) => {
            use audioplumbing::ChunkTrait;
            json::stringify(status_info.last_chunk.get_ro_envelope())
        }
        Err(why) => {
            println!("error getting status in fn json_chunk(): {:?}", why);
            "".to_string()
        }
    }
}

#[get("/json/envelope")]
pub fn json_envelope(controller: State<audioplumbing::Controller<f32>>) -> String {
    match controller.get_envelope() {
        Ok(envelope) => {
            let mut data = json::JsonValue::new_array();
            for i in envelope.iter() {
                data.push(*i).unwrap();
            }
            data.dump()
        }
        Err(why) => {
            println!("error getting status in fn json_chunk(): {:?}", why);
            "".to_string()
        }
    }
}

#[get("/hello_world")] // <- route attribute
pub fn hi_world() -> &'static str {
    "hello, world!"
}

#[get("/dart/<file..>")]
pub fn dart_files(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("dart/build/").join(file)).ok()
}
