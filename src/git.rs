use std::path::PathBuf;
use std::process::Command;

use eframe::egui::{self, RichText};

use crate::theme;

pub struct GitView {
    root: PathBuf,
    branch: String,
    files: Vec<(String, String)>,
    log: Vec<String>,
    selected: Option<String>,
    diff: String,
    error: Option<String>,
    loaded: bool,
}

impl GitView {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            branch: String::new(),
            files: Vec::new(),
            log: Vec::new(),
            selected: None,
            diff: String::new(),
            error: None,
            loaded: false,
        }
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    fn refresh(&mut self) {
        self.loaded = true;
        self.error = None;
        match self.git(&["status", "--porcelain=v1", "-b", "-uall"]) {
            Ok(s) => {
                let mut lines = s.lines();
                self.branch = lines.next().unwrap_or("").trim_start_matches("## ").to_string();
                self.files = lines
                    .filter(|l| l.len() > 3)
                    .map(|l| (l[..2].to_string(), l[3..].to_string()))
                    .collect();
            }
            Err(e) => {
                self.error = Some(e);
                self.files.clear();
                self.log.clear();
                return;
            }
        }
        self.log = self
            .git(&["log", "--oneline", "--decorate", "-n", "50"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(f) = self.selected.clone() {
            self.load_diff(&f);
        }
    }

    fn load_diff(&mut self, file: &str) {
        let path = file.rsplit(" -> ").next().unwrap_or(file);
        let mut diff = self.git(&["diff", "HEAD", "--", path]).unwrap_or_default();
        if diff.is_empty() {
            // Untracked: show the file contents as an all-added diff.
            diff = std::fs::read_to_string(self.root.join(path))
                .map(|c| c.lines().map(|l| format!("+{l}\n")).collect())
                .unwrap_or_else(|_| "(binary or unreadable file)".into());
        }
        self.diff = diff;
        self.selected = Some(file.to_string());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.loaded {
            self.refresh();
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("branch: {}", self.branch)).strong());
            if ui.button("Refresh").clicked() {
                self.refresh();
            }
        });
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::p().red));
            return;
        }
        ui.separator();

        let mut clicked = None;
        egui::Panel::left("git-files").frame(egui::Frame::NONE).resizable(true).default_size(320.0).show(ui, |ui| {
            ui.label(RichText::new(format!("Changes ({})", self.files.len())).strong());
            egui::ScrollArea::vertical().id_salt("git-file-list").max_height(ui.available_height() * 0.55).show(ui, |ui| {
                for (status, file) in &self.files {
                    let color = match status.trim() {
                        "M" | "MM" => theme::p().yellow,
                        "A" | "??" => theme::p().green,
                        "D" => theme::p().red,
                        _ => ui.visuals().text_color(),
                    };
                    let text = RichText::new(format!("{status} {file}")).monospace().color(color);
                    if ui.selectable_label(self.selected.as_deref() == Some(file), text).clicked() {
                        clicked = Some(file.clone());
                    }
                }
            });
            ui.separator();
            ui.label(RichText::new("History").strong());
            egui::ScrollArea::vertical().id_salt("git-log").show(ui, |ui| {
                for l in &self.log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        });
        if let Some(f) = clicked {
            self.load_diff(&f);
        }

        egui::ScrollArea::both().id_salt("git-diff").auto_shrink(false).show(ui, |ui| {
            if self.selected.is_none() {
                ui.label(RichText::new("Select a file to see its diff.").weak());
            }
            for line in self.diff.lines() {
                let color = if line.starts_with('+') && !line.starts_with("+++") {
                    theme::p().green
                } else if line.starts_with('-') && !line.starts_with("---") {
                    theme::p().red
                } else if line.starts_with("@@") {
                    theme::p().blue
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(line).monospace().color(color));
            }
        });
    }
}
