use dotenv::dotenv;
use std::env;

pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Tip,
}

pub fn log(message: &str, level: LogLevel) {
    dotenv().ok();
    let set_level_str = env::var("RUNPOD_DEBUG_LEVEL").unwrap_or_else(|_| String::from("DEBUG"));
    let set_level = match set_level_str.to_uppercase().as_str() {
        "ERROR" => LogLevel::Error,
        "WARN" => LogLevel::Warn,
        "INFO" => LogLevel::Info,
        "DEBUG" => LogLevel::Debug,
        _ => LogLevel::Debug,
    };

    let runpod_debug = env::var("RUNPOD_DEBUG").unwrap_or_else(|_| String::from("true"));
    if runpod_debug.to_lowercase() != "true" {
        return;
    }

    match level {
        LogLevel::Error if matches!(set_level, LogLevel::Error) => eprintln!("ERROR  | {}", message),
        LogLevel::Warn if matches!(set_level, LogLevel::Error | LogLevel::Warn) => eprintln!("WARN   | {}", message),
        LogLevel::Info if matches!(set_level, LogLevel::Error | LogLevel::Warn | LogLevel::Info) => eprintln!("INFO   | {}", message),
        LogLevel::Debug if matches!(set_level, LogLevel::Error | LogLevel::Warn | LogLevel::Info | LogLevel::Debug) => eprintln!("DEBUG  | {}", message),
        LogLevel::Tip => eprintln!("TIP    | {}", message),
        _ => (),
    }
}

pub fn log_secret(secret_name: &str, secret: Option<String>, level: LogLevel) {
    let pod_id = env::var("RUNPOD_POD_ID");
    if pod_id.is_ok() {
        if let Some(secret_value) = secret {
            let redacted_secret = format!("{}{}{}", &secret_value[..1], "*".repeat(secret_value.len() - 2), &secret_value[secret_value.len() - 1..]);
            log(&format!("{}: {}", secret_name, redacted_secret), level);
        } else {
            log(&format!("{}: Could not read environment variable.", secret_name), LogLevel::Error);
        }
    }
}

pub fn error(message: &str) {
    log(message, LogLevel::Error);
}

pub fn warn(message: &str) {
    log(message, LogLevel::Warn);
}

pub fn info(message: &str) {
    log(message, LogLevel::Info);
}

pub fn debug(message: &str) {
    log(message, LogLevel::Debug);
}

pub fn tip(message: &str) {
    log(message, LogLevel::Tip);
}

fn main() {
    log_secret("RUNPOD_AI_API_KEY", env::var("RUNPOD_AI_API_KEY").ok(), LogLevel::Info);
    log_secret("RUNPOD_WEBHOOK_GET_JOB", env::var("RUNPOD_WEBHOOK_GET_JOB").ok(), LogLevel::Info);
    log_secret("RUNPOD_WEBHOOK_POST_OUTPUT", env::var("RUNPOD_WEBHOOK_POST_OUTPUT").ok(), LogLevel::Info);
}
