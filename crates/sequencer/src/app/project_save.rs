//! Background project saves.
//!
//! A save splits in two. [`App::prepare_project_capture`] runs on the main
//! thread and takes what the save reads; the pattern pools are `Arc`-shared,
//! so that part is cheap. Flattening the capture into a [`ProjectFile`],
//! encoding it, writing it and dropping it are the expensive part (seconds
//! for a project with many rack clips) and run on a worker thread, so the UI
//! keeps running while a save is in flight.
//!
//! Saves land in the order they were requested: each worker builds its file
//! in parallel with the previous one but waits for it before writing.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use super::history::SaveMark;
use super::projects::ProjectCapture;
use super::App;
use crate::project;

/// How one save ended. Already applied to the app (saved revision, project
/// name, recent list) by the time the caller sees it.
pub struct ProjectSaveOutcome {
    pub name: String,
    pub result: Result<PathBuf, String>,
    mark: SaveMark,
}

pub struct ProjectSaves {
    /// The most recently started worker. Every worker joins its predecessor
    /// before writing, so joining this one waits for all of them.
    latest: Option<JoinHandle<()>>,
    sender: mpsc::Sender<ProjectSaveOutcome>,
    receiver: mpsc::Receiver<ProjectSaveOutcome>,
    /// Started saves whose outcome has not been applied yet.
    in_flight: usize,
    /// The newest started save while any is in flight.
    pending: Option<PendingSave>,
}

struct PendingSave {
    name: String,
    mark: SaveMark,
}

impl Default for ProjectSaves {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            latest: None,
            sender,
            receiver,
            in_flight: 0,
            pending: None,
        }
    }
}

impl Drop for ProjectSaves {
    /// Never let process exit cut a save short.
    fn drop(&mut self) {
        if let Some(handle) = self.latest.take() {
            let _ = handle.join();
        }
    }
}

impl ProjectSaves {
    fn start(&mut self, name: String, capture: ProjectCapture, mark: SaveMark) {
        let previous = self.latest.take();
        let sender = self.sender.clone();
        self.pending = Some(PendingSave {
            name: name.clone(),
            mark,
        });
        self.in_flight += 1;
        let handle = std::thread::Builder::new()
            .name("project-save".to_string())
            .stack_size(crate::REQUIRED_THREAD_STACK_SIZE)
            .spawn(move || {
                let project = capture.build();
                if let Some(previous) = previous {
                    let _ = previous.join();
                }
                let result = match &project {
                    Ok(project) => {
                        project::save_project(&name, project).map_err(|error| error.to_string())
                    }
                    Err(error) => Err(error.clone()),
                };
                let _ = sender.send(ProjectSaveOutcome { name, result, mark });
                // Freeing the file's tree takes a while; report first.
                drop(project);
            })
            .expect("spawn project-save thread");
        self.latest = Some(handle);
    }
}

impl App {
    /// Start saving the project under `requested_name` (or its current
    /// name) and return the name it saves as. The file is written on a
    /// worker thread; [`Self::poll_project_saves`] reports when it lands.
    pub fn begin_project_save(&mut self, requested_name: Option<&str>) -> Result<String, String> {
        let requested_name = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                // A "save as" still writing has already renamed the project.
                self.project_saves
                    .pending
                    .as_ref()
                    .filter(|pending| self.history.is_same_project(pending.mark))
                    .map(|pending| pending.name.clone())
            })
            .or_else(|| self.current_project_name.clone())
            .ok_or_else(|| "Project name must contain letters or numbers".to_string())?;

        let save_name = project::sanitize_project_name(&requested_name);
        if save_name.is_empty() {
            return Err("Project name must contain letters or numbers".to_string());
        }

        super::edit::finish_active_gesture(self);
        let capture = self.prepare_project_capture(&save_name)?;
        let mark = self.history.save_mark();
        self.project_saves.start(save_name.clone(), capture, mark);
        Ok(save_name)
    }

    pub fn project_save_in_flight(&self) -> bool {
        self.project_saves.in_flight > 0
    }

    /// Whether an in-flight save captured exactly the current state, so the
    /// project counts as saved while that save is still writing.
    pub(super) fn current_state_is_being_saved(&self) -> bool {
        self.project_saves
            .pending
            .as_ref()
            .is_some_and(|pending| self.history.is_at(pending.mark))
    }

    /// Saves that finished since the last call, without blocking.
    pub fn poll_project_saves(&mut self) -> Vec<ProjectSaveOutcome> {
        let outcomes: Vec<_> = self.project_saves.receiver.try_iter().collect();
        self.apply_project_save_outcomes(outcomes)
    }

    /// Block until every started save has landed, and return their outcomes.
    pub fn wait_for_project_saves(&mut self) -> Vec<ProjectSaveOutcome> {
        if let Some(handle) = self.project_saves.latest.take() {
            let _ = handle.join();
        }
        self.poll_project_saves()
    }

    fn apply_project_save_outcomes(
        &mut self,
        outcomes: Vec<ProjectSaveOutcome>,
    ) -> Vec<ProjectSaveOutcome> {
        for outcome in &outcomes {
            self.project_saves.in_flight = self.project_saves.in_flight.saturating_sub(1);
            if outcome.result.is_ok() {
                // A save that lands after another project was opened wrote
                // its own project's file; it must not rename the open one.
                if self.history.mark_saved_at(outcome.mark) {
                    self.current_project_name = Some(outcome.name.clone());
                }
                if let Err(error) = crate::recent_projects::record(&outcome.name) {
                    eprintln!("Could not update recent projects: {error}");
                }
            }
        }
        if self.project_saves.in_flight == 0 {
            self.project_saves.pending = None;
        }
        outcomes
    }

    /// Save and wait for the write: [`Self::begin_project_save`] followed by
    /// [`Self::wait_for_project_saves`]. Returns every outcome that landed,
    /// this save's last.
    pub fn save_project_and_wait(
        &mut self,
        requested_name: Option<&str>,
    ) -> Result<Vec<ProjectSaveOutcome>, String> {
        self.begin_project_save(requested_name)?;
        Ok(self.wait_for_project_saves())
    }

    /// [`Self::save_project_and_wait`] reduced to this save's own result.
    pub fn save_project_with_name(
        &mut self,
        requested_name: Option<&str>,
    ) -> Result<String, String> {
        let outcome = self
            .save_project_and_wait(requested_name)?
            .pop()
            .expect("the save just started reports an outcome");
        outcome.result.map(|_| outcome.name)
    }
}
