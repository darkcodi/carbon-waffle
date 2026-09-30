mod app;
mod command;
mod model;
mod runner;

fn main() -> iced::Result {
    if std::env::args().any(|arg| arg == "--worker") {
        if let Err(error) = runner::worker_main() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    iced::application(app::App::new, app::App::update, app::App::view)
        .title("Carbon Waffle · Wireless workbench")
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1280.0, 900.0),
            min_size: Some(iced::Size::new(1000.0, 720.0)),
            exit_on_close_request: false,
            ..Default::default()
        })
        .run()
}
