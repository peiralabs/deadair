#![forbid(unsafe_code)]

mod config;
mod probe;
mod serve;
mod verdict;

use config::{Config, Thresholds};
use probe::Probe;
use std::collections::VecDeque;
use std::env;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use verdict::{Level, Observation, Verdict, evaluate};

const HELP: &str = "deadair — verify a gluetun and qBittorrent stack\n\nUsage:\n  deadair check [--json]\n  deadair watch\n  deadair explain\n  deadair --help\n";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn marker(level: Level) -> &'static str {
    match level {
        Level::Ok => "[ok]",
        Level::Warn => "[warn]",
        Level::Fail => "[fail]",
    }
}
fn exit_code(level: Level) -> i32 {
    match level {
        Level::Ok => 0,
        Level::Warn => 1,
        Level::Fail => 2,
    }
}
fn report(verdict: &Verdict) {
    let width = verdict
        .checks
        .iter()
        .map(|item| item.name.len())
        .max()
        .unwrap_or(0);
    for item in &verdict.checks {
        println!(
            "{:<6} {:<width$}  {}",
            marker(item.level),
            item.name,
            item.detail
        );
    }
    println!("{:<6} overall", marker(verdict.level));
}
fn one(config: &Config) -> Verdict {
    let mut probe = Probe::new(config.clone());
    // A one-shot run has no history to confirm against, so hysteresis would downgrade
    // every genuine failure to a warning and `check` could never exit 2.
    let thresholds = Thresholds {
        confirmations: 1,
        ..config.thresholds
    };
    evaluate(&[probe.observe(now())], &thresholds)
}
fn check(config: &Config, json: bool) -> i32 {
    let verdict = one(config);
    if json {
        println!(
            "{}",
            serde_json::to_string(&verdict).expect("serializing verdict")
        );
    } else {
        report(&verdict);
    }
    exit_code(verdict.level)
}
fn explain(config: &Config) {
    let verdict = one(config);
    let mut explained = false;
    for item in verdict.checks.iter().filter(|item| item.level != Level::Ok) {
        println!("{}:\n{}\n", item.name, item.remedy);
        explained = true;
    }
    if !explained {
        println!("Nothing to explain: all checks pass.");
    }
}
fn watch(config: Config) -> ! {
    let started = now();
    let state = Arc::new(Mutex::new(serve::Snapshot {
        verdict: evaluate(&[], &config.thresholds),
        observation: None,
        started,
    }));
    let server_state = Arc::clone(&state);
    let listen = config.listen.clone();
    thread::spawn(move || {
        if let Err(error) = serve::serve(&listen, server_state) {
            eprintln!("metrics server: {error}");
        }
    });
    println!(
        "deadair watch started; metrics listening on {}",
        config.listen
    );
    let samples_for_window = config
        .thresholds
        .stall_window
        .div_ceil(config.interval.max(1)) as usize
        + 1;
    let capacity = samples_for_window
        .saturating_add(config.thresholds.confirmations)
        .max(8);
    let mut history: VecDeque<Observation> = VecDeque::with_capacity(capacity);
    let mut previous = None;
    let mut probe = Probe::new(config.clone());
    loop {
        history.push_back(probe.observe(now()));
        while history.len() > capacity {
            history.pop_front();
        }
        let verdict = evaluate(history.make_contiguous(), &config.thresholds);
        let level = verdict.level;
        if previous != Some(level) {
            println!("{} overall", marker(level));
            previous = Some(level);
        }
        *state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = serve::Snapshot {
            verdict,
            observation: history.back().cloned(),
            started,
        };
        thread::sleep(Duration::from_secs(config.interval));
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = Config::from_env();
    match args.as_slice() {
        [command] if command == "check" => std::process::exit(check(&config, false)),
        [command, flag] if command == "check" && flag == "--json" => {
            std::process::exit(check(&config, true));
        }
        [command] if command == "watch" => watch(config),
        [command] if command == "explain" => explain(&config),
        _ => print!("{HELP}"),
    }
}
