// This file provides interop with the langserver
//
use algorithm;

use base64;
use serde::{Deserialize, Serialize};
use serde_json;

use algorithmia::error::{Error, ErrorKind, ResultExt};
use algorithmia::prelude::{AlgoIo, EntryPoint};
use serde_json::Value;
use std::error::Error as StdError;
use std::fs::OpenOptions;
use std::io::{self, BufRead, Write};
use std::process;

const ALGOOUT: &'static str = "/tmp/algoout";

#[derive(Deserialize)]
struct Request {
    data: Value,
    content_type: String,
}

#[derive(Serialize)]
struct AlgoSuccess {
    result: Value,
    metadata: RunnerMetadata,
}

#[derive(Serialize)]
struct AlgoFailure {
    error: RunnerError,
}

#[derive(Serialize)]
struct RunnerMetadata {
    content_type: String,
}

#[derive(Serialize)]
struct RunnerError {
    message: String,
    error_type: &'static str,
}

impl AlgoSuccess {
    fn new<S: Into<String>>(result: Value, content_type: S) -> AlgoSuccess {
        AlgoSuccess {
            result: result,
            metadata: RunnerMetadata {
                content_type: content_type.into(),
            },
        }
    }
}

impl AlgoFailure {
    fn new(err: &dyn StdError) -> AlgoFailure {
        AlgoFailure {
            error: RunnerError {
                message: error_cause_chain(err),
                error_type: "AlgorithmError",
            },
        }
    }

    fn system(err: &dyn StdError) -> AlgoFailure {
        AlgoFailure {
            error: RunnerError {
                message: error_cause_chain(err),
                error_type: "SystemError",
            },
        }
    }
}

fn main() {
    let mut algo = algorithm::Algo::default();
    println!("PIPE_INIT_COMPLETE");
    flush_std_pipes();

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let output_json = match line {
            Ok(input) => {
                let output = call_algorithm(&mut algo, input);
                flush_std_pipes();
                serialize_output(output)
            }
            Err(_) => {
                let err = line.chain_err(|| "failed to read stdin").unwrap_err();
                serde_json::to_string(&AlgoFailure::system(&err as &dyn StdError))
                    .expect("Failed to encode JSON")
            }
        };
        algoout(&output_json);
    }
}

impl From<AlgoIo> for AlgoSuccess {
    fn from(output: AlgoIo) -> AlgoSuccess {
        match output {
            AlgoIo::Text(text) => AlgoSuccess::new(Value::String(text), "text"),
            AlgoIo::Json(json_obj) => AlgoSuccess::new(json_obj, "json"),
            AlgoIo::Binary(bytes) => {
                let result = base64::encode(&bytes);
                AlgoSuccess::new(Value::String(result), "binary")
            }
        }
    }
}

fn error_cause_chain(err: &dyn StdError) -> String {
    let mut causes = vec![err.to_string()];
    let mut e = err;
    while let Some(cause) = e.source() {
        causes.push(cause.to_string());
        e = cause;
    }
    causes.join("\ncaused by: ")
}

fn serialize_output(output: Result<AlgoIo, Box<dyn StdError>>) -> String {
    let json_result = match output {
        Ok(output) => serde_json::to_string(&AlgoSuccess::from(output)),
        Err(err) => serde_json::to_string(&AlgoFailure::new(&*err as &dyn StdError)),
    };

    json_result.expect("Failed to encode JSON")
}

fn flush_std_pipes() {
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
}

fn algoout(output_json: &str) {
    match OpenOptions::new().write(true).open(ALGOOUT) {
        Ok(mut f) => {
            let _ = f.write(output_json.as_bytes());
            let _ = f.write(b"\n");
        }
        Err(e) => {
            println!("Cannot write to algoout pipe: {}\n", e);
            process::exit(-1);
        }
    };
}

fn call_algorithm<E: EntryPoint>(algo: &mut E, stdin: String) -> Result<AlgoIo, Box<dyn StdError>> {
    let req = serde_json::from_str(&stdin).chain_err(|| ErrorKind::DecodeJson("request"))?;
    let Request { data, content_type } = req;
    let input = match (&*content_type, data) {
        ("text", Value::String(text)) => AlgoIo::Text(text),
        ("binary", Value::String(ref encoded)) => {
            let bytes =
                base64::decode(encoded).chain_err(|| ErrorKind::DecodeBase64("request input"))?;
            AlgoIo::Binary(bytes)
        }
        ("json", json_obj) => AlgoIo::Json(json_obj),
        (_, _) => return Err(Error::from(ErrorKind::InvalidContentType(content_type)).into()),
    };
    algo.apply(input)
}