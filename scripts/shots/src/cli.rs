//! The shots checker's command line: its subcommands and their arguments.
//! What a subcommand does is in lib.rs and the modules it calls, so a change
//! to an argument's name or help text changes neither version of the checker
//! (builds' build_source.rs lists this file as unversioned): the pictures and
//! runs builds stores stay valid.

use crate::names::Prefix;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "builds shots")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) action: Action,
}

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Name the canvas runtime copy that draws the boards.
    Runtime,
    /// Start the pinned Chrome and print one line, `chrome=started <version>`,
    /// `chrome=no-address <why>` (it ran and printed no debugging address in
    /// its deadline) or `chrome=failed <why>`, so that builds can tell before
    /// a build whether its shots can draw boards on this host.
    Chrome,
    /// Print the checker's capture version and comparison version, which each
    /// run's findings.json names.
    Version,
    /// Take the shots into shots/, a folder for each app, and check them.
    /// With --before, only pairs worse against that run fail the run,
    /// and the rest are listed; without it, every pair with a finding fails.
    Run {
        #[command(flatten)]
        selection: Selection,
        /// The findings.json of the run to compare with, written by this
        /// version of the checker, such as the run of the branch's merge base
        /// with origin/main, which builds passes.
        #[arg(long, conflicts_with = "base")]
        before: Option<PathBuf>,
        /// Compare with an empty run of this commit, the branch's merge base,
        /// whose run checked no scene: every pair is new, so its findings are
        /// listed and fail nothing. builds passes it for a prefix that starts
        /// no scene the merge base declares.
        #[arg(long, conflicts_with_all = ["before", "base"])]
        empty_before: Option<String>,
        /// The folder of the board pictures earlier runs drew, which builds
        /// keeps: the run draws only the boards it lacks and adds each new
        /// drawing.
        #[arg(long)]
        pictures: Option<PathBuf>,
        /// The app source tree the shots are taken of, which builds names:
        /// when the picture folder keeps every selected shot of an app
        /// for it, the run takes none and uses those, and otherwise it keeps
        /// the shots it takes there.
        #[arg(long, requires = "pictures")]
        app_tree: Option<String>,
        /// Fail instead of capturing when the picture folder lacks a selected
        /// shot of the app tree: builds passes it when it built no app.
        #[arg(long, requires = "app_tree")]
        no_capture: bool,
        /// Take every selected shot again, even when the picture folder keeps
        /// it for the app tree, and replace the kept pictures with the new
        /// ones. A shot taken before its window had loaded stays wrong in the
        /// folder until a run takes it again.
        #[arg(long, requires = "app_tree", conflicts_with = "no_capture")]
        retake: bool,
        /// Take every selected shot into the picture folder, mark each
        /// selected prefix taken, and end without drawing boards or comparing
        /// anything: builds takes the shots of all a lane's prefixes in one
        /// run, then runs each prefix with --no-capture.
        #[arg(
            long,
            requires = "app_tree",
            conflicts_with_all = ["before", "no_capture", "base"]
        )]
        capture_only: bool,
    },
    /// Print the number of shots a run of the same selection takes, which
    /// builds counts to give the run a time limit that grows with its work.
    Scenes {
        #[command(flatten)]
        selection: Selection,
    },
    /// Take the shots into a directory.
    Capture {
        #[command(flatten)]
        selection: Selection,
        /// The directory the PNGs and element lists are written to.
        #[arg(long)]
        out: PathBuf,
    },
    /// Check shots already taken against their boards.
    Check {
        /// The directory holding every app's PNGs and element lists.
        #[arg(long)]
        shots: PathBuf,
        /// The directory the report is written to, with the rendered boards.
        #[arg(long)]
        out: PathBuf,
        /// The commit the shots were taken at, for the report.
        #[arg(long)]
        sha: String,
    },
    /// Compare again the pairs a report holds, with this checker, and print
    /// the findings as records (the report's findings.json). The report is
    /// a cached run's folder, which holds each pair's pictures and element
    /// lists. The command draws and captures nothing and reads no repo, so a
    /// change to the comparison shows what it clears and adds on a run that
    /// another checker made.
    Recheck {
        /// The report's folder, with its findings.json and its `img` folder.
        #[arg(long)]
        report: PathBuf,
    },
    /// Write a platform's harness, the code that takes an app's shots inside
    /// the app's own tests, into a folder that those tests compile. The command writes the folder whole, with a .gitignore that
    /// keeps it out of the app's commits, and leaves a folder that already
    /// holds this harness untouched. It refuses a folder that holds other
    /// files.
    Harness {
        /// The platform whose harness to write: mac or android.
        platform: String,
        /// The folder to write, such as <app>/shots-harness.
        #[arg(long)]
        into: PathBuf,
    },
    /// Fail when a declared shot has no board.
    Lint,
    /// List each design-system component state that a board draws and no row
    /// of the component's preview shows: a value of a state prop, or a
    /// component no preview row draws, with the boards that draw it. A prop
    /// is part of a state when it is true, or a code word (a lower-case
    /// letter, then letters, digits, - and _) that the component never draws
    /// as words; children, callbacks, numbers, words, className and icon
    /// names (icon, name, or a prop that ends in Icon) are not. With
    /// --against, list only the states that the listing at that revision
    /// does not hold, and fail when there is one.
    States(StatesArgs),
    /// Print whether each named board or component preview draws alike at
    /// the two revisions. Give no selectors to compare every board and
    /// component preview from either revision.
    Draws(DrawsArgs),
    /// List each design-system component's preview rows whose shot its
    /// capture file (Components/<Component>.<extension> under the app's
    /// declare folder) does not declare, and each capture file with no
    /// preview. A component, or a row (`Component.row`), that
    /// `shots-design-only` names for an app is left out there and counted.
    /// The listing fails nothing.
    Components {
        /// A component to show with every row, such as MacRow; repeat for
        /// several. With none, every component with a row its capture file
        /// does not declare.
        components: Vec<String>,
    },
}

#[derive(Args)]
pub(crate) struct Selection {
    /// An app to take shots of; repeat for several. With none, the apps of
    /// the --select prefixes, or every app the repo's .app-build gives a
    /// shots-capture line.
    #[arg(long = "app")]
    pub(crate) apps: Vec<String>,
    /// A board-name prefix: `mac.welcome.` is the Mac welcome boards,
    /// `mac.component.row.` one component's states. Repeat for several. With
    /// none, every shot the app declares.
    #[arg(long = "select", value_parser = Prefix::parse)]
    pub(crate) prefixes: Vec<Prefix>,
    /// Also take the shots that take the keyboard focus, which bring the app to
    /// the front.
    #[arg(long)]
    pub(crate) windows: bool,
    /// This is the run of the branch's merge base: a --select prefix that
    /// starts no declared shot selects none, because the branch adds those
    /// shots, and the run has no pairs for them. Without it, such a prefix
    /// fails the run.
    #[arg(long)]
    pub(crate) base: bool,
}

/// The arguments of `states`.
#[derive(Args)]
pub(crate) struct StatesArgs {
    /// Draw the boards and previews as they are at this revision, not as
    /// the working tree holds them.
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// List only the states that the listing at this revision does not
    /// hold, and fail when there is one.
    #[arg(long)]
    pub(crate) against: Option<String>,
    /// The folder of the board pictures and instance lists earlier runs kept,
    /// which builds passes: a board or preview it keeps is not drawn again,
    /// and one it lacks is drawn and kept there.
    #[arg(long)]
    pub(crate) pictures: Option<PathBuf>,
}

/// The arguments of `draws`.
#[derive(Args)]
pub(crate) struct DrawsArgs {
    /// The first revision.
    #[arg(long)]
    pub(crate) at: String,
    /// The revision to compare with.
    #[arg(long)]
    pub(crate) against: String,
    /// A board's name (`design/mocks/<name>.dc.html`); repeat for several.
    #[arg(long = "board")]
    pub(crate) boards: Vec<String>,
    /// A component's name, whose preview page is drawn; repeat for several.
    #[arg(long = "component")]
    pub(crate) components: Vec<String>,
    /// The folder of the board pictures earlier runs drew, which builds
    /// keeps and passes: a screen board takes its drawing from it by its
    /// board hash when it keeps one, and a board it lacks is drawn and
    /// kept there.
    #[arg(long)]
    pub(crate) pictures: Option<PathBuf>,
}
