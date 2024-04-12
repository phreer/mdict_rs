use bytes::Bytes;
use mdict_index::{MDictAsyncLookup, MDictSqliteIndex};
use regex::Regex;
use std::{
    env, fmt::Write as _, fs::File, io::{stderr, Read, Write}, path::{Path, PathBuf}, sync::Arc
};
use serde::Serialize;
use tinytemplate::TinyTemplate;
use warp::{filters::path::Tail, http::Response, Filter};
use tokio::io::AsyncReadExt;

static MDICT_RESULT_HTML: &'static str = include_str!("../static/html/result.html");

fn usage(program: &str) {
    let usage = format!("Usage: {} config-file port\n", program);
    stderr().write(usage.as_bytes()).unwrap();
}

#[derive(Serialize)]
struct MDictContent {
    title: String,
    index: usize,
    contents: Vec<String>,
}

#[derive(Serialize)]
struct MDictContents {
    mdict_contents: Vec<MDictContent>,
}

fn fix_content(content: String, i: usize) -> String {
    let content = Regex::new(r#"(src|href)\s*=\s*"(file://|sound:/|entry:/)?/?([^"]+)""#)
        .unwrap()
        .replace_all(&content, |link: &regex::Captures| {
            if link[3].contains("data:") {
                return link[0].to_string()
            }
            match link.get(2) {
                Some(m) => {
                    let proto = m.as_str();
                    match proto {
                        "sound:/" => format!(r#"{}="sound://{}/{}""#,&link[1], i, &link[3]),
                        "entry:/" => format!(r#"{}="/{}""#,&link[1], &link[3]),
                        _ =>format!(r#"{}="/{}/{}""#,&link[1], i, &link[3])
                    }
                }
                None => format!(r#"{}="/{}/{}""#,&link[1], i, &link[3])
            }
        });
    let content = Regex::new("@@@LINK=([^\\s]+)")
        .unwrap()
        .replace_all(&content, |link: &regex::Captures| {
            format!(
                "<a href=\"/{}\" >See also: {}</a>",
                urlencoding::encode(&link[1]), &link[1]
            )
        });
    content.into()
}

#[tokio::main]
async fn main() {
    let arg0 = env::args().nth(0).unwrap();
    let config_path = env::args().nth(1).unwrap_or_else(|| {
        usage(&arg0);
        std::process::exit(-1);
        #[allow(unreachable_code)]
        "".to_string()
    }).to_owned();
    let mut config_file = File::open(&config_path).unwrap();
    let mut config = String::new();
    let server_port = env::args().nth(2).unwrap_or_else(|| {
        usage(&arg0);
        std::process::exit(-1);
        #[allow(unreachable_code)]
        "".to_string()
    }).parse::<u16>().expect("invalid argument for port");
    config_file.read_to_string(&mut config).unwrap();
    if env::var_os("RUST_LOG").is_none() {
        env::set_var(
            "RUST_LOG",
            "warn,mdict=info,mdict_index=info,main=info,warp=info",
        );
    }
    pretty_env_logger::init();
    let log = warp::log("main");
    let mut indexes = Vec::new();
    let mut paths = Vec::new();
    for path in config.lines() {
        let mdict = MDictSqliteIndex::new(&path).await.unwrap();
        indexes.push(mdict);
        let dir = Path::new(&path)
            .canonicalize()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        paths.push(dir);
    }
    let indexes = Arc::new(indexes);
    let paths = Arc::new(paths);
    let indexes_clone = indexes.clone();
    let indexes_shared = warp::any().map(move || indexes_clone.clone());
    let indexes_shared2 = warp::any().map(move || indexes.clone());
    let paths_shared = warp::any().map(move || paths.clone());
    let mdict_server = warp::path::param()
        .and(warp::path::tail())
        .and(indexes_shared)
        .and_then(
            |i: usize, path: Tail, mdict: Arc<Vec<MDictSqliteIndex>>| async move {
                if i >= mdict.len() {
                    return Err(warp::reject::not_found());
                }
                let path = path.as_str();
                log::info!("load: {:?}/{:?}", i, path);
                let mime = mime_guess::from_path(path)
                    .first()
                    .unwrap_or(mime::TEXT_HTML_UTF_8);
                match mdict[i].lookup_resource(path).await {
                    Ok(mut data) => {
                        if mime == mime::TEXT_CSS || mime == mime::TEXT_CSS_UTF_8 {
                            data = fix_css(i, data);
                        }
                        Ok(Response::builder()
                            .header("content-type", mime.to_string())
                            .body(data.to_vec())
                            .unwrap())
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            log::error!("load {} failed : {}", path, e);
                        }
                        Err(warp::reject::not_found())
                    }
                }
            },
        );
    let files = warp::path!(usize / String)
        .and(warp::path::end())
        .and(paths_shared)
        .and_then(
            |i: usize, uri: String, paths: Arc<Vec<PathBuf>>| async move {
                if i >= paths.len() {
                    return Err(warp::reject::not_found());
                }
                log::info!("load files: {:?}/{:?}", i, uri);
                let mut file = paths[i].clone();
                file.push(&uri);
                if file.exists() {
                    let mut file = tokio::fs::File::open(&file)
                        .await
                        .map_err(|_| warp::reject::not_found())?;
                    let mut data = Vec::new();
                    file.read_to_end(&mut data)
                        .await
                        .map_err(|_| warp::reject::not_found())?;
                    let mime = mime_guess::from_path(uri).first();
                    let mime = mime.unwrap_or(mime::TEXT_HTML_UTF_8);
                    let data = if mime == mime::TEXT_CSS || mime == mime::TEXT_CSS_UTF_8 {
                        fix_css(i, data.into())
                    } else {
                        data.into()
                    };
                    Ok(Response::builder()
                        .header("content-type", mime.to_string())
                        .body(data.to_vec())
                        .unwrap())
                } else {
                    Err(warp::reject::not_found())
                }
            },
        );
    let static_files = warp::path::path("static")
        .and(warp::path::tail())
        .and(warp::path::end())
        .and_then(
            |uri: Tail| async move {
                let file_path = std::path::PathBuf::from("static/".to_string() + uri.as_str());
                if file_path.exists() {
                    log::info!("load: {:?}", file_path);
                    let mut file = tokio::fs::File::open(&file_path)
                        .await
                        .map_err(|_| warp::reject::not_found())?;
                    let mut data = Vec::new();
                    file.read_to_end(&mut data)
                        .await
                        .map_err(|_| warp::reject::not_found())?;
                    let mime = mime_guess::from_path(file_path).first();
                    let mime = mime.unwrap_or(mime::TEXT_HTML_UTF_8);
                    Ok(Response::builder().
                        header("content-type", mime.to_string())
                        .body(data.to_vec())
                        .unwrap())
                } else {
                    Err(warp::reject::not_found())
                }
            },
        );
    let lookup = warp::path::param()
        .and(warp::path::end())
        .and(indexes_shared2)
        .and_then(
            |keyword: String, mdict: Arc<Vec<MDictSqliteIndex>>| async move {
                let key = urlencoding::decode(&keyword).unwrap();
                log::info!("lookup: {:?}", key);
                let mut no_result = true;
                let mut mdict_contents = Vec::new();
                for (i, dict) in mdict.iter().enumerate() {
                    let result = dict.lookup_word(&key).await;
                    let contents = match result {
                        Ok(result) => result,
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::NotFound {
                                log::error!("lookup {} failed : {}", key, e);
                            }
                            continue;
                        },
                    };
                    no_result = false;
                    let contents = contents.into_iter().map(|s| fix_content(s, i) ).collect();
                    mdict_contents.push(MDictContent{
                        title: dict.header.attrs.get("Title").unwrap_or(&"Unknown dictionary".to_string()).to_owned(),
                        index: i,
                        contents
                    });
                }
                let mdict_contents = MDictContents { mdict_contents };
                let mut tt = TinyTemplate::new();
                tt.set_default_formatter(&tinytemplate::format_unescaped);
                tt.add_template("result", MDICT_RESULT_HTML).expect("failed to add template for result");
                let body = format!("{}", tt.render("result", &mdict_contents).unwrap());
                if no_result {
                    return Err(warp::reject::not_found())
                }
                Ok(warp::reply::html(body))
            },
        );
    let routes = warp::get().and(files).or(static_files).or(mdict_server).or(lookup).with(log);
    warp::serve(routes).run(([0, 0, 0, 0], server_port)).await;
}

// from flask-mdict
fn fix_css(id: usize, css: Bytes) -> Bytes {
    let css = std::str::from_utf8(&css).unwrap();
    // remove comments, https://stackoverflow.com/questions/9329552/explain-regex-that-finds-css-comments
    let css = Regex::new(r#"(/\*[^*]*\*+([^/*][^*]*\*+)*/)"#)
        .unwrap()
        .replace_all(&css, "");
    let css =
        Regex::new(r#"\s*([^}/;]+?)\s*\{"#)
            .unwrap()
            .replace_all(&css, |caps: &regex::Captures| {
                let tags = &caps[1];
                if tags.starts_with("@") {
                    caps[0].to_string()
                } else {
                    let mut result = "\n".to_string();
                    for tag in tags.split(',') {
                        let tag = tag.trim();
                        write!(&mut result, "#mdict_rs_{} {},", id, tag).unwrap();
                    }
                    result.pop();
                    result.push('{');
                    result
                }
            });
    css.to_string().into()
}

// TODO: cache page using lru
// TODO: build regex using once_cell
