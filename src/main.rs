mod app;
mod command;
mod model;
mod monitor;
mod pattern;
mod runner;

fn main() -> iced::Result {
    if std::env::args().any(|arg| arg == "--session-worker") {
        if let Err(error) = runner::session_worker_main() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--worker") {
        if let Err(error) = runner::worker_main() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    iced::application(app::App::boot, app::App::update, app::App::view)
        .title("Carbon Waffle · Wireless workbench")
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1120.0, 850.0),
            min_size: Some(iced::Size::new(900.0, 680.0)),
            exit_on_close_request: false,
            ..Default::default()
        })
        .run()
}
