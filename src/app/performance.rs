//! Opt-in rendering benchmark. Uses the actual screens, synthetic data, and an
//! in-memory software framebuffer; never opens a window or runs radio commands.
use super::*;
use iced::{Event, Font, Point, Rectangle, Size, mouse};
use iced_runtime::{UserInterface, core, user_interface};
use iced_tiny_skia::graphics::Viewport;
use std::time::Instant;

fn populated_app() -> App {
    let mut app = App::new(true);
    app.interfaces = vec![Interface {
        name: "wlan0mon".into(),
        phy: "phy0".into(),
        monitor: true,
    }];
    app.interface = app.interfaces.first().cloned();
    app.load_demo();
    let template = app.survey.networks[0].clone();
    app.survey.networks = (0..250)
        .map(|i| model::Network {
            bssid: format!("02:00:00:00:{:02X}:{:02X}", i / 256, i % 256),
            ssid: format!("Research network {i}"),
            ..template.clone()
        })
        .collect();
    app.survey.stations = (0..500)
        .map(|i| model::Station {
            mac: format!("02:01:00:00:{:02X}:{:02X}", i / 256, i % 256),
            bssid: app.survey.networks[i / 2].bssid.clone(),
            power: -45,
            packets: 100,
        })
        .collect();
    app
}

#[test]
#[ignore = "manual rendering benchmark; reports timings instead of hardware-dependent thresholds"]
fn scroll_rendering() {
    let mut app = populated_app();
    app.panels.dependencies = true;
    for (page, grouped) in [
        (Page::Elevate, false),
        (Page::Discover, false),
        (Page::Discover, true),
    ] {
        app.page = page;
        if grouped {
            for (index, network) in app.survey.networks.iter_mut().enumerate() {
                network.ssid = format!("Research network {}", index / 4);
                app.expanded_networks.insert(network.ssid.clone());
            }
        }
        let size = Size::new(900.0, 680.0);
        let viewport = Viewport::with_physical_size(Size::new(900, 680), 1.0);
        let mut renderer = iced_tiny_skia::Renderer::new(Font::DEFAULT, 16.0.into());
        let mut pixels = tiny_skia::Pixmap::new(900, 680).unwrap();
        let mut mask = tiny_skia::Mask::new(900, 680).unwrap();
        let mut cache = user_interface::Cache::default();
        let cursor = mouse::Cursor::Available(Point::new(450.0, 400.0));
        let theme = app.theme();
        let mut layout_times = Vec::new();
        let mut frame_times = Vec::new();
        for frame in 0..25 {
            let start = Instant::now();
            let mut ui = UserInterface::build(app.view(), size, cache, &mut renderer);
            let layout = start.elapsed();
            let mut messages = Vec::new();
            let (_, statuses) = ui.update(
                &[Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels {
                        x: 0.0,
                        y: if frame % 2 == 0 { -12.0 } else { 12.0 },
                    },
                })],
                cursor,
                &mut renderer,
                &mut core::clipboard::Null,
                &mut messages,
            );
            assert!(statuses.contains(&core::event::Status::Captured));
            ui.draw(
                &mut renderer,
                &theme,
                &core::renderer::Style {
                    text_color: appearance::FOREGROUND,
                },
                cursor,
            );
            renderer.draw(
                &mut pixels.as_mut(),
                &mut mask,
                &viewport,
                &[Rectangle::with_size(size)],
                appearance::BACKGROUND,
            );
            let elapsed = start.elapsed();
            cache = ui.into_cache();
            if frame >= 5 {
                layout_times.push(layout.as_secs_f64() * 1000.0);
                frame_times.push(elapsed.as_secs_f64() * 1000.0);
            }
        }
        layout_times.sort_by(f64::total_cmp);
        frame_times.sort_by(f64::total_cmp);
        eprintln!(
            "{page:?}{}: layout median {:.2} ms; scroll + full software redraw median {:.2} ms, p95 {:.2} ms",
            if grouped {
                " (expanded SSID groups)"
            } else {
                ""
            },
            layout_times[10],
            frame_times[10],
            frame_times[18],
        );
    }
}

#[test]
#[ignore = "manual Capture screen previews; writes software-rendered PPM files under target/capture-previews"]
fn capture_previews() {
    let directory = PathBuf::from("target/capture-previews");
    fs::create_dir_all(&directory).unwrap();
    for (name, state) in [
        ("ready", None),
        ("recording", None),
        ("checking", None),
        ("found", Some(Inspection::Found)),
        ("missing", Some(Inspection::NotFound)),
        ("error", Some(Inspection::Unknown)),
        ("options", None),
    ] {
        let mut app = App::new(true);
        app.page = Page::Capture;
        app.interface.as_mut().unwrap().monitor = true;
        app.load_demo();
        app.target.as_mut().unwrap().ssid = "Research Wi-Fi".into();
        app.status = "Select Start recording to begin.".into();
        if name != "ready" {
            app.capture_path = "/home/researcher/captures/session/capture-4-01.cap".into();
            app.capture_available = true;
        }
        if name == "recording" || name == "options" {
            let operation = app.operation(Action::Capture).unwrap();
            let mut job = app.runner.start(1, operation, true).unwrap();
            job.started = true;
            app.jobs.push(job);
            app.capture_started = Some(Instant::now() - Duration::from_secs(65));
            app.status = "Recording traffic from the selected network.".into();
            app.panels.reconnect = name == "options";
        } else if name == "checking" {
            let operation = app.operation(Action::Inspect).unwrap();
            app.jobs.push(app.runner.start(1, operation, true).unwrap());
            app.status = "Checking the saved recording.".into();
        }
        app.capture_check = state;
        // Render real-mode copy using only synthetic data and demo jobs.
        app.demo = false;
        for (width, height) in [(900, 680), (1120, 850)] {
            render_preview(
                &app,
                &directory.join(format!("{name}-{width}.ppm")),
                width,
                height,
            );
        }
    }
}

#[test]
#[ignore = "manual Recover previews; writes software-rendered PPM files under target/recover-previews"]
fn recover_previews() {
    let directory = PathBuf::from("target/recover-previews");
    fs::create_dir_all(&directory).unwrap();
    for name in [
        "empty",
        "loaded",
        "dictionary",
        "pattern",
        "preview",
        "syntax",
        "invalid",
        "running",
        "hashcat",
    ] {
        let mut app = App::new(true);
        app.flow = Flow::Recover;
        app.recovery_capture_path = "/home/researcher/captures/capture-4-01.cap".into();
        app.recovery_bssid = "02:00:00:00:00:01".into();
        app.hash_path = "/home/researcher/captures/handshake.hc22000".into();
        app.wordlist = "/home/researcher/wordlists/words.txt".into();
        app.target = None;
        app.interface = None;
        app.interfaces.clear();
        app.authorization = Authorization::Idle;
        app.status = "Ready to recover the password.".into();
        if !matches!(name, "dictionary" | "empty") {
            let _ = app.update(Message::RecoveryMode(RecoveryMode::Pattern));
        }
        if name == "empty" {
            app.recovery_capture_path.clear();
            app.recovery_bssid.clear();
            app.hash_path.clear();
            app.wordlist.clear();
        }
        if name == "loaded" {
            app.recovery_networks = vec![crate::runner::CaptureNetwork {
                bssid: app.recovery_bssid.clone(),
                ssid: "Research Wi-Fi".into(),
                handshakes: 1,
            }];
        }
        if name == "preview" {
            app.pattern_preview = Some(Ok(pattern::Preview {
                total: 4_000,
                words: 2,
                samples: vec![
                    "AlphaAlpha000".into(),
                    "AlphaAlpha001".into(),
                    "AlphaAlpha002".into(),
                ],
            }));
        }
        app.panels.pattern_help = name == "syntax";
        if name == "invalid" {
            let _ = app.update(Message::Pattern("{Word}{Word}[0-9]+".into()));
        }
        if name == "running" {
            let operation = app.operation(Action::Crack).unwrap();
            let mut job = app.runner.start(1, operation, true).unwrap();
            job.started = true;
            app.jobs.push(job);
            app.candidate_progress = Some((2048, "4,000".into()));
        }
        if name == "hashcat" {
            app.engine = Engine::Hashcat;
        }
        app.demo = false;
        for (width, height) in [(900, 680), (1120, 850)] {
            render_preview(
                &app,
                &directory.join(format!("{name}-{width}.ppm")),
                width,
                height,
            );
        }
    }
}

fn render_preview(app: &App, path: &std::path::Path, width: u32, height: u32) {
    let size = Size::new(width as f32, height as f32);
    let mut renderer = iced_tiny_skia::Renderer::new(Font::DEFAULT, 16.0.into());
    let mut pixels = tiny_skia::Pixmap::new(width, height).unwrap();
    let mut mask = tiny_skia::Mask::new(width, height).unwrap();
    let mut ui = UserInterface::build(
        app.view(),
        size,
        user_interface::Cache::default(),
        &mut renderer,
    );
    let mut messages = Vec::new();
    let _ = ui.update(
        &[Event::Window(iced::window::Event::RedrawRequested(
            Instant::now(),
        ))],
        mouse::Cursor::Unavailable,
        &mut renderer,
        &mut core::clipboard::Null,
        &mut messages,
    );
    ui.draw(
        &mut renderer,
        &app.theme(),
        &core::renderer::Style {
            text_color: appearance::FOREGROUND,
        },
        mouse::Cursor::Unavailable,
    );
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &Viewport::with_physical_size(Size::new(width, height), 1.0),
        &[Rectangle::with_size(size)],
        appearance::BACKGROUND,
    );
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for pixel in pixels.pixels() {
        // The renderer's software surface uses BGR for presentation.
        ppm.extend([pixel.blue(), pixel.green(), pixel.red()]);
    }
    fs::write(path, ppm).unwrap();
}
