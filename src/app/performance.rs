//! Opt-in rendering benchmark. Uses the actual screens, synthetic data, and an
//! in-memory software framebuffer; never opens a window or runs radio commands.
use super::*;
use iced::{Event, Font, Point, Rectangle, Size, mouse};
use iced_runtime::{UserInterface, core, user_interface};
use iced_tiny_skia::graphics::Viewport;
use std::time::Instant;

fn populated_app() -> App {
    let mut app = App::new();
    app.demo = true;
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
    for page in [Page::Monitoring, Page::Discover] {
        app.page = page;
        app.panels.tools = true;
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
            "{page:?}: layout median {:.2} ms; scroll + full software redraw median {:.2} ms, p95 {:.2} ms",
            layout_times[10], frame_times[10], frame_times[18],
        );
    }
}
