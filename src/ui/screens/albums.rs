use crate::{
    mpd::{client::Client, commands::Song as MpdSong, mpd_client::Filter, mpd_client::MpdClient},
    state::State,
    ui::{widgets::browser::Browser, KeyHandleResult, Level, SharedUiState, StatusMessage},
};

use super::{
    browser::{StringOrSong, ToListItems},
    dirstack::DirStack,
    CommonAction, Screen,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{prelude::Rect, widgets::ListItem, Frame};
use tracing::instrument;

#[derive(Debug)]
pub struct AlbumsScreen {
    stack: DirStack<StringOrSong>,
    position: CurrentPosition,
    filter_input_mode: bool,
}

impl Default for AlbumsScreen {
    fn default() -> Self {
        Self {
            stack: DirStack::new(Vec::new()),
            position: CurrentPosition::Album(Position { values: Album }),
            filter_input_mode: false,
        }
    }
}

impl AlbumsScreen {
    #[instrument]
    async fn prepare_preview(&mut self, client: &mut Client<'_>, state: &State) -> Result<Vec<ListItem<'static>>> {
        let idx = self
            .stack
            .current()
            .1
            .get_selected()
            .context("Expected an item to be selected")?;
        let current = &self.stack.current().0[idx];
        Ok(match &self.position {
            CurrentPosition::Album(val) => val
                .fetch(client, current.to_current_value())
                .await?
                .to_listitems(&state.config.symbols),
            CurrentPosition::Song(val) => val
                .fetch(client, current.to_current_value())
                .await?
                .to_listitems(&state.config.symbols),
        })
    }
}

#[async_trait]
impl Screen for AlbumsScreen {
    type Actions = AlbumsActions;

    fn render<B: ratatui::prelude::Backend>(
        &mut self,
        frame: &mut Frame<B>,
        area: Rect,
        app: &mut State,
        _shared_state: &mut SharedUiState,
    ) -> Result<()> {
        let w = Browser::new(&app.config.symbols, &app.config.column_widths);
        frame.render_stateful_widget(w, area, &mut self.stack);

        Ok(())
    }

    #[instrument(err)]
    async fn before_show(
        &mut self,
        _client: &mut Client<'_>,
        _app: &mut crate::state::State,
        _shared: &mut SharedUiState,
    ) -> Result<()> {
        match _client.list_tag("album", None).await.context("Cannot list tags")? {
            Some(result) => {
                self.stack = DirStack::new(result.0.into_iter().map(StringOrSong::Dir).collect());
                self.position = CurrentPosition::default();
                self.stack.next = self
                    .prepare_preview(_client, _app)
                    .await
                    .context("Cannot prepare preview")?;
            }
            None => {
                _shared.status_message = Some(StatusMessage::new("No albums found!".to_owned(), Level::Info));
            }
        };

        Ok(())
    }

    async fn handle_action(
        &mut self,
        event: KeyEvent,
        client: &mut Client<'_>,
        app: &mut State,
        shared: &mut SharedUiState,
    ) -> Result<KeyHandleResult> {
        if self.filter_input_mode {
            match event.code {
                KeyCode::Char(c) => {
                    if let Some(ref mut f) = self.stack.filter {
                        f.push(c);
                    };
                    Ok(KeyHandleResult::RenderRequested)
                }
                KeyCode::Backspace => {
                    if let Some(ref mut f) = self.stack.filter {
                        f.pop();
                    };
                    Ok(KeyHandleResult::RenderRequested)
                }
                KeyCode::Enter => {
                    self.filter_input_mode = false;
                    self.stack.jump_forward();
                    Ok(KeyHandleResult::RenderRequested)
                }
                KeyCode::Esc => {
                    self.filter_input_mode = false;
                    self.stack.filter = None;
                    Ok(KeyHandleResult::RenderRequested)
                }
                _ => Ok(KeyHandleResult::SkipRender),
            }
        } else if let Some(action) = app.config.keybinds.albums.get(&event.into()) {
            match action {
                _ => Ok(KeyHandleResult::SkipRender),
            }
        } else if let Some(action) = app.config.keybinds.navigation.get(&event.into()) {
            match action {
                CommonAction::DownHalf => {
                    self.stack.next_half_viewport();
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::UpHalf => {
                    self.stack.prev_half_viewport();
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Up => {
                    self.stack.prev();
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Down => {
                    self.stack.next();
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Bottom => {
                    self.stack.last();
                    self.prepare_preview(client, app).await?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Top => {
                    self.stack.first();
                    self.prepare_preview(client, app).await?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Right => {
                    let idx = self
                        .stack
                        .current()
                        .1
                        .get_selected()
                        .context("Expected an item to be selected")?;
                    let current = self.stack.current().0[idx].to_current_value().to_owned();
                    self.position = match &mut self.position {
                        CurrentPosition::Album(val) => {
                            self.stack.push(val.fetch(client, &current).await?);
                            CurrentPosition::Song(val.next(current))
                        }
                        CurrentPosition::Song(val) => {
                            val.add_to_queue(client, &current).await?;
                            shared.status_message = Some(StatusMessage::new(
                                format!("'{}' from album '{}' added to queue", current, val.values.album),
                                Level::Info,
                            ));
                            CurrentPosition::Song(val.next())
                        }
                    };
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::Left => {
                    self.stack.pop();
                    self.position = match &mut self.position {
                        CurrentPosition::Album(val) => CurrentPosition::Album(val.prev()),
                        CurrentPosition::Song(val) => CurrentPosition::Album(val.prev()),
                    };
                    self.stack.next = self
                        .prepare_preview(client, app)
                        .await
                        .context("Cannot prepare preview")?;
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::EnterSearch => {
                    self.filter_input_mode = true;
                    self.stack.filter = Some(String::new());
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::NextResult => {
                    self.stack.jump_forward();
                    Ok(KeyHandleResult::RenderRequested)
                }
                CommonAction::PreviousResult => {
                    self.stack.jump_back();
                    Ok(KeyHandleResult::RenderRequested)
                }
            }
        } else {
            Ok(KeyHandleResult::KeyNotHandled)
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum AlbumsActions {}

#[derive(Debug)]
struct Album;
#[derive(Debug)]
struct Song {
    album: String,
}
#[derive(Debug)]
struct Position<T> {
    values: T,
}
#[derive(Debug)]
enum CurrentPosition {
    Album(Position<Album>),
    Song(Position<Song>),
}

impl Default for CurrentPosition {
    fn default() -> Self {
        Self::Album(Position { values: Album })
    }
}

impl Position<Album> {
    fn next(&mut self, album: String) -> Position<Song> {
        Position { values: Song { album } }
    }

    fn prev(&mut self) -> Position<Album> {
        Position { values: Album }
    }

    async fn fetch(&self, client: &mut Client<'_>, value: &str) -> Result<Vec<StringOrSong>> {
        Ok(client
            .list_tag("title", Some(&[Filter { tag: "album", value }]))
            .await?
            .map_or(Vec::new(), |v| v.0.into_iter().map(StringOrSong::Song).collect()))
    }
}

impl Position<Song> {
    fn next(&mut self) -> Position<Song> {
        Position {
            values: Song {
                album: std::mem::take(&mut self.values.album),
            },
        }
    }
    fn prev(&mut self) -> Position<Album> {
        Position { values: Album }
    }

    async fn fetch(&self, client: &mut Client<'_>, value: &str) -> Result<MpdSong> {
        let mut res = client
            .find(&[
                Filter { tag: "title", value },
                Filter {
                    tag: "album",
                    value: &self.values.album,
                },
            ])
            .await?
            .unwrap_or_else(|| crate::mpd::commands::Songs(Vec::new()))
            .0;
        Ok(std::mem::take(&mut res[0]))
    }

    async fn add_to_queue(&self, client: &mut Client<'_>, value: &str) -> Result<()> {
        Ok(client
            .find_add(&[
                Filter { tag: "title", value },
                Filter {
                    tag: "album",
                    value: &self.values.album,
                },
            ])
            .await?)
    }
}