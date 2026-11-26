use serenity::all::{CreateEmbed, CreateEmbedFooter};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub struct CardData {
    pub title: String,
    pub description: String,
    pub color: u32,
    pub footer: String,
}
impl CardData {
    pub fn embed(&self) -> CreateEmbed {
        CreateEmbed::new()
            .title(&self.title)
            .description(&self.description)
            .color(self.color)
            .footer(CreateEmbedFooter::new(&self.footer))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Todo,
    Active,
    Done,
    Failed,
}
pub struct Card {
    steps: Vec<(&'static str, &'static str, State)>,
    title: &'static str,
    emoji: Option<String>,
    note: Option<String>,
    detail: Option<&'static str>,
    status: State,
    frame: usize,
    start: Instant,
    elapsed: Option<f64>,
}
impl Card {
    pub fn new(from_url: bool, emoji: Option<String>) -> Self {
        let mut steps = Vec::new();
        if from_url {
            steps.extend([
                ("validate", "Validating URL", State::Todo),
                ("download", "Downloading content", State::Todo),
            ]);
        }
        steps.extend([
            ("detect", "Detecting obfuscator", State::Todo),
            ("deliver", "Preparing download", State::Todo),
        ]);
        Self {
            steps,
            title: if from_url {
                "Fetching script"
            } else {
                "Processing script"
            },
            emoji,
            note: None,
            detail: None,
            status: State::Active,
            frame: 0,
            start: Instant::now(),
            elapsed: None,
        }
    }
    pub fn begin(&mut self, key: &str) {
        if self.status != State::Active || !self.steps.iter().any(|s| s.0 == key) {
            return;
        }
        let mut seen = false;
        for (k, _, state) in &mut self.steps {
            if *k == key {
                *state = State::Active;
                seen = true;
            } else {
                *state = if seen { State::Todo } else { State::Done };
            }
        }
        self.note = None;
        self.detail = None;
    }
    pub fn stage(&mut self, name: &str) {
        let detail = match name {
            "analyse" => "reading the loader",
            "unpack" => "opening the VM",
            "classify" => "recognizing handlers",
            "decompile" => "rebuilding the code",
            "verify" => "verifying the result",
            "clean" => "cleaning up the result",
            "sandbox" => "running it in the fake Roblox",
            _ => return,
        };
        self.begin("detect");
        if self.status == State::Active {
            self.detail = Some(detail);
        }
    }
    pub fn queued(&mut self, position: usize) {
        if self.status == State::Active {
            self.note = Some(format!("Waiting in the queue (position {position})."));
        }
    }
    pub fn complete(&mut self) {
        if self.status == State::Active {
            for s in &mut self.steps {
                s.2 = State::Done;
            }
            self.stop(State::Done);
        }
    }
    pub fn fail(&mut self) {
        if self.status != State::Active {
            return;
        }
        let i = self
            .steps
            .iter()
            .position(|s| s.2 == State::Active)
            .or_else(|| self.steps.iter().position(|s| s.2 == State::Todo));
        if let Some(i) = i {
            self.steps[i].2 = State::Failed;
        }
        self.stop(State::Failed);
    }
    fn stop(&mut self, state: State) {
        self.status = state;
        self.note = None;
        self.elapsed = Some(self.start.elapsed().as_secs_f64());
    }
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
    pub fn data(&self) -> CardData {
        let spin = self.emoji.clone().unwrap_or_else(|| {
            ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][self.frame % 10].into()
        });
        let elapsed = self
            .elapsed
            .unwrap_or_else(|| self.start.elapsed().as_secs_f64());
        let (title, color, intro, footer) = match self.status {
            State::Done => (
                "✅ Result ready".into(),
                0x57f287,
                "All steps finished.",
                format!("Finished in {elapsed:.1}s"),
            ),
            State::Failed => (
                "❌ Failed".into(),
                0xed4245,
                "The job stopped. The reason is in the message.",
                format!("Stopped after {elapsed:.1}s"),
            ),
            _ => (
                format!("{spin} {}", self.title),
                0x4e5058,
                "Working on your sample.",
                format!("{elapsed:.1}s elapsed"),
            ),
        };
        let lines = self
            .steps
            .iter()
            .map(|(_, label, state)| match state {
                State::Active => format!(
                    "**{spin} {label}**{}",
                    self.detail.map(|s| format!(" · {s}")).unwrap_or_default()
                ),
                State::Done => format!("✅ {label}"),
                State::Failed => format!("❌ {label}"),
                State::Todo => format!("▫️ {label}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        CardData {
            title,
            color,
            description: format!(
                "{intro}{}\n\n{lines}",
                self.note
                    .as_ref()
                    .map(|s| format!("\n{s}"))
                    .unwrap_or_default()
            ),
            footer,
        }
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn elapsed_footers_format_tenths_for_active_success_and_failure() {
        for (state, elapsed, expected) in [
            (State::Active, 0.0, "0.0s elapsed"),
            (State::Active, 1.5, "1.5s elapsed"),
            (State::Done, 3.5, "Finished in 3.5s"),
            (State::Failed, 4.0, "Stopped after 4.0s"),
        ] {
            let mut card = Card::new(false, None);
            card.status = state;
            card.elapsed = Some(elapsed);
            assert_eq!(card.data().footer, expected);
        }
    }

    #[test]
    fn terminal_cards_keep_captured_duration_when_start_and_progress_change() {
        for failed in [false, true] {
            let mut card = Card::new(false, None);
            card.begin("detect");
            card.start = Instant::now() - Duration::from_millis(3500);
            if failed {
                card.fail();
            } else {
                card.complete();
            }
            let elapsed = card
                .elapsed
                .expect("terminal state did not capture elapsed time");
            assert!(elapsed >= 3.5);
            let stopped = card.data();
            card.start = Instant::now() - Duration::from_secs(1000);
            card.tick();
            card.queued(7);
            card.begin("deliver");
            card.stage("verify");
            card.complete();
            card.fail();
            assert_eq!(card.elapsed, Some(elapsed));
            assert_eq!(card.data(), stopped);
        }
    }
}
