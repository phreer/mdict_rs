//! # mdict_dump
//!
//! This simple program load mdx and mdd files and then print all keywords in
//! mdx file to stdout.
//!
//! # Usage
//!
//! ```shell
//! mdict_dump [PATH TO MDX FILE]
//! ```
//!
//! # panic
//!
//! This program will panic if the mdx file is invalid or can't be opened by `mdict`

use mdict_index::*;
use std::io::{stderr, Write};
use std::path::Path;
use std::env;
use std::process::exit;

fn usage(program: &str) {
    let USAGE = format!("\
    Usage: {} command args\n\
    \n\
    Available commands:\n\
    \tkey:       print all keys\n\
    \tsearch:    search and dump the content of a key\n\
    ", program);
    stderr().write(USAGE.as_bytes()).unwrap();
}

fn do_keys(args: Vec<String>) {
    let file = &args[0];
    let mdx_file = Path::new(file).canonicalize().unwrap();
    let index = MDictMemIndex::new(mdx_file).unwrap();
    for i in index.keyword_iter() {
        println!("{}", i);
    }
}

async fn do_search(args: Vec<String>) {
    let file = &args.get(0).expect("MDX file is required");
    let key = &args.get(1).expect("key for search is required");

    let mdx_file = Path::new(file).canonicalize().unwrap();
    let index = MDictMemIndex::new(mdx_file).unwrap();
    if let Some(result) = index.lookup_word(key).await.ok() {
        println!("Content:\n{result}");
    } else {
        println!("not found");
    }
}

#[tokio::main]
async fn main() {
    if env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "info");
    }
    pretty_env_logger::init();
    let program = env::args().nth(0).unwrap();
    let command = env::args().nth(1).expect("command is required");
    match command.as_str() {
        "key" => do_keys(env::args().skip(2).collect()),
        "search" => do_search(env::args().skip(2).collect()).await,
        _ => {
            println!("unknown command {command}");
            usage(program.as_str());
            exit(-1);
        }
    }
}
