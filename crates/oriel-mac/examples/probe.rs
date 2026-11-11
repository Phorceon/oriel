//! Drives `MacPlatform` through the public `Platform` trait and prints every event.
//!
//! See crates/oriel-mac/README.md for the sub-commands.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use oriel_core::{AppInfo, Command, Event, Platform, Rect, WindowId};
use oriel_mac::MacPlatform;
use std::io::BufRead;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

fn parse_rect(a: &[&str]) -> Option<Rect> {
    let n: Vec<f64> = a.iter().take(4).filter_map(|s| s.parse().ok()).collect();
    (n.len() == 4).then(|| Rect::new(n[0], n[1], n[2], n[3]))
}

fn wid(s: &str) -> Option<WindowId> {
    s.parse().ok().map(WindowId)
}

/// Parse one repl line. `None` for a bad line.
fn parse_line(line: &str) -> Option<Command> {
    let p: Vec<&str> = line.split_whitespace().collect();
    let cmd = *p.first()?;
    let b = || p.get(1).map(|s| s.to_string());
    Some(match cmd {
        "launch" => {
            let path = p.get(1)?;
            Command::Launch {
                app: AppInfo { bundle_id: String::new(), name: path.to_string(), path: path.into() },
            }
        }
        "present" => Command::Present { bundle_id: b()?, window: wid(p.get(2)?)?, frame: parse_rect(&p[3..])? },
        "frame" => Command::SetFrame { window: wid(p.get(1)?)?, frame: parse_rect(&p[2..])? },
        "hide" => Command::HideApp { bundle_id: b()? },
        "show" => Command::ShowApp { bundle_id: b()? },
        "min" => Command::Minimize { window: wid(p.get(1)?)? },
        "close" => Command::CloseWindow { window: wid(p.get(1)?)? },
        "quit" => Command::QuitApp { bundle_id: b()? },
        "reopen" => Command::Reopen { bundle_id: b()? },
        "watch" => Command::Watch { bundle_id: b()? },
        "unwatch" => Command::Unwatch { bundle_id: b()? },
        "scan" => Command::ScanApps,
        "request-trust" => Command::RequestTrust,
        _ => return None,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    let Some(mtm) = MainThreadMarker::new() else { return };

    let platform = Arc::new(MacPlatform::new());
    let (tx, rx) = channel::<Event>();
    let quiet = matches!(mode, "trust" | "apps");
    platform.start(Box::new(move |e| {
        if !quiet {
            println!("{e:?}");
        }
        let _ = tx.send(e);
    }));

    match mode {
        "trust" | "apps" | "request-trust" if mode != "request-trust" => {
            let want_apps = mode == "apps";
            if want_apps {
                platform.send(Command::ScanApps);
            }
            let deadline = Duration::from_secs(10);
            while let Ok(e) = rx.recv_timeout(deadline) {
                match (mode, e) {
                    ("trust", Event::TrustChanged(t)) => {
                        println!("{}", if t { "trusted" } else { "not trusted" });
                        return;
                    }
                    ("apps", Event::AppsScanned(apps)) => {
                        for a in apps {
                            println!("{}\t{}\t{}", a.bundle_id, a.name, a.path.display());
                        }
                        return;
                    }
                    _ => {}
                }
            }
            eprintln!("timeout");
        }
        "request-trust" | "watch" | "repl" => {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            if mode == "request-trust" {
                platform.send(Command::RequestTrust);
            } else {
                for b in rest {
                    platform.send(Command::Watch { bundle_id: b.clone() });
                }
            }
            if mode == "repl" {
                let p = platform.clone();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        if let Some(secs) = line.strip_prefix("sleep ") {
                            std::thread::sleep(Duration::from_secs_f64(secs.trim().parse().unwrap_or(0.5)));
                        } else if line == "exit" {
                            break;
                        } else if line == "flush" {
                            let t = std::time::Instant::now();
                            p.flush(Duration::from_secs(5));
                            println!("flushed after {} ms", t.elapsed().as_millis());
                        } else if let Some(c) = parse_line(line) {
                            println!("> {line}");
                            p.send(c);
                        } else {
                            eprintln!("bad line: {line}");
                        }
                    }
                    // Let the last commands run, then leave.
                    std::thread::sleep(Duration::from_secs(1));
                    std::process::exit(0);
                });
            }
            app.run();
        }
        _ => eprintln!("usage: probe trust | request-trust | apps | watch <bundle>... | repl <bundle>..."),
    }
}
