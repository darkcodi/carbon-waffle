//! An iced file chooser: no GTK, desktop portal, or external dialog dependency.
use super::*;
use appearance as style;
use iced::{
    Center, Element, Fill,
    widget::{button, column, container, row, text, text_input},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Capture,
    Wordlist,
}

#[derive(Debug, Clone)]
pub struct File {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}

pub(super) struct Browser {
    pub purpose: Purpose,
    pub directory: PathBuf,
    pub files: Vec<File>,
    pub hidden: bool,
    pub filter: String,
    pub loading: bool,
    pub error: Option<String>,
}

pub(super) fn list(
    directory: &std::path::Path,
    purpose: Purpose,
    hidden: bool,
) -> Result<Vec<File>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        // Follow links in a user-navigated browser, never in an automatic walk.
        let Ok(metadata) = fs::metadata(entry.path()) else {
            continue;
        };
        let directory = metadata.is_dir();
        if directory
            || (metadata.is_file()
                && (purpose == Purpose::Wordlist || library::supported(&entry.path())))
        {
            files.push(File {
                path: entry.path(),
                name,
                directory,
            });
        }
    }
    files.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(files)
}

impl App {
    pub(super) fn browse(&mut self, purpose: Purpose) -> Task<Message> {
        if self.closing || self.offline_busy() || self.library.importing {
            return Task::none();
        }
        let home = library::home().unwrap_or_else(|| PathBuf::from("/"));
        let directory = if purpose == Purpose::Wordlist {
            absolute_path(&self.wordlist)
                .ok()
                .and_then(|p| p.parent().map(PathBuf::from))
                .unwrap_or(home)
        } else {
            home
        };
        self.browser = Some(Browser {
            purpose,
            directory: directory.clone(),
            files: vec![],
            hidden: false,
            filter: String::new(),
            loading: false,
            error: None,
        });
        self.browse_directory(directory)
    }

    pub(super) fn browse_directory(&mut self, directory: PathBuf) -> Task<Message> {
        let Some(browser) = &mut self.browser else {
            return Task::none();
        };
        self.browser_revision += 1;
        let revision = self.browser_revision;
        browser.directory = directory.clone();
        browser.files.clear();
        browser.filter.clear();
        browser.loading = true;
        browser.error = None;
        let purpose = browser.purpose;
        let hidden = browser.hidden;
        Task::batch([
            background(
                move || list(&directory, purpose, hidden),
                move |result| Message::BrowserLoaded(revision, result),
            ),
            iced::widget::operation::scroll_to(
                "current-step",
                iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
            ),
        ])
    }

    pub(super) fn file_browser(&self, browser: &Browser) -> Element<'_, Message> {
        let mut shortcuts = row![].spacing(8);
        if let Some(home) = self
            .library
            .root
            .parent()
            .and_then(|p| p.parent())
            .map(PathBuf::from)
        {
            for (name, path) in [
                ("Home", home.clone()),
                ("Downloads", home.join("Downloads")),
                ("Saved captures", self.library.root.clone()),
            ] {
                shortcuts = shortcuts.push(
                    button(text(name).size(12))
                        .on_press(Message::BrowserDirectory(path))
                        .padding([8, 12])
                        .style(style::secondary),
                );
            }
        }
        if let Some(legacy) = &self.legacy_captures {
            shortcuts = shortcuts.push(
                button(text("Previous captures").size(12))
                    .on_press(Message::BrowserDirectory(legacy.clone()))
                    .padding([8, 12])
                    .style(style::secondary),
            );
        }
        let mut content = column![
            row![
                text(if browser.purpose == Purpose::Capture {
                    "Import a capture"
                } else {
                    "Choose a wordlist"
                })
                .size(25)
                .font(style::SEMIBOLD),
                iced::widget::space().width(Fill),
                button(text("Cancel").size(13))
                    .on_press(Message::BrowserCancel)
                    .style(style::secondary)
                    .padding([10, 16])
            ]
            .align_y(Center),
            text(if browser.purpose == Purpose::Capture {
                "Choose a capture or WPA hash file. A copy will be added to your library."
            } else {
                "Choose the text file containing your passwords or source words."
            })
            .size(13)
            .color(style::MUTED),
            shortcuts.wrap(),
            row![
                button(text("↑ Up").size(12))
                    .on_press_maybe(
                        browser
                            .directory
                            .parent()
                            .map(|p| Message::BrowserDirectory(p.into()))
                    )
                    .padding([8, 12])
                    .style(style::secondary),
                text(browser.directory.to_string_lossy().into_owned())
                    .size(12)
                    .color(style::MUTED)
                    .width(Fill)
            ]
            .align_y(Center)
            .spacing(10),
            row![
                text_input("Filter files and folders…", &browser.filter)
                    .on_input(Message::BrowserFilter)
                    .size(13)
                    .padding(10)
                    .style(style::input),
                button(
                    text(if browser.hidden {
                        "Hide hidden"
                    } else {
                        "Show hidden"
                    })
                    .size(12)
                )
                .on_press(Message::BrowserHidden)
                .padding([10, 12])
                .style(style::secondary)
            ]
            .align_y(Center)
            .spacing(8),
        ]
        .spacing(14)
        .width(Fill);
        if browser.loading {
            content = content.push(text("Reading folder…").size(13).color(style::MUTED));
        }
        if let Some(error) = &browser.error {
            content = content.push(text(error.clone()).size(13).color(style::ERROR));
        }
        let filter = browser.filter.to_lowercase();
        let mut count = 0;
        for file in browser
            .files
            .iter()
            .filter(|f| f.name.to_lowercase().contains(&filter))
        {
            count += 1;
            content = content.push(
                button(
                    row![
                        text(if file.directory { "Folder" } else { "File" })
                            .size(11)
                            .color(style::MUTED)
                            .width(48),
                        text(file.name.clone()).size(14).width(Fill),
                        text(if file.directory { "→" } else { "Choose" }).size(12),
                    ]
                    .spacing(12)
                    .align_y(Center),
                )
                .width(Fill)
                .padding([12, 14])
                .style(style::secondary)
                .on_press(if file.directory {
                    Message::BrowserDirectory(file.path.clone())
                } else {
                    Message::BrowserChoose(file.path.clone())
                }),
            );
        }
        if count == 0 && !browser.loading && browser.error.is_none() {
            content = content.push(
                text("No matching files in this folder.")
                    .size(13)
                    .color(style::MUTED),
            );
        }
        container(content).max_width(720).width(Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_filters_captures_but_allows_any_wordlist_and_folder_navigation() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["capture.cap", "saved.hc22000", "words.txt", ".hidden.cap"] {
            fs::write(directory.path().join(name), b"data").unwrap();
        }
        fs::create_dir(directory.path().join("nested")).unwrap();
        let captures = list(directory.path(), Purpose::Capture, false).unwrap();
        assert_eq!(captures.len(), 3);
        assert!(captures[0].directory);
        assert_eq!(
            list(directory.path(), Purpose::Capture, true)
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            list(directory.path(), Purpose::Wordlist, false)
                .unwrap()
                .len(),
            4
        );
    }
}
