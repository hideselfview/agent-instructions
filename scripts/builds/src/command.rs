//! Every builds command, declared once: the words that name it and its usage
//! lines. `Command::parse` reads a command line into a `Command`, for this
//! process and for the command lines of other builds processes that health
//! and the orphan check read. Each question builds asks about a command
//! (which stores it uses, whether it keeps the Mac awake, which gates it
//! passes) is a method of `Command` or `Parsed` in the module that answers
//! it, with a match that names every command, so a new command does not
//! compile until it answers each one.

use crate::fail;
use crate::fail::Result;

macro_rules! declare {
    (
        leaves {
            $(
                $(#[$leaf_doc:meta])*
                $leaf:ident => [$([$($leaf_word:literal),+]),+], [$($leaf_usage:literal),* $(,)?];
            )+
        }
        groups {
            $(
                $group:ident($module:ident::$sub:ident) {
                    $(
                        $(#[$doc:meta])*
                        $variant:ident => [$([$($word:literal),+]),+], [$($usage:literal),* $(,)?];
                    )+
                }
            )+
        }
    ) => {
        /// A builds command: a command of its own, or a group's subcommand.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Command {
            $( $(#[$leaf_doc])* $leaf, )+
            $( $group($module::$sub), )+
        }

        $(
            /// The subcommands of one builds command.
            pub mod $module {
                #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
                pub enum $sub {
                    $( $(#[$doc])* $variant, )+
                }

                pub use $sub::*;

                impl $sub {
                    pub(super) fn words(self) -> &'static [&'static [&'static str]] {
                        match self {
                            $( $sub::$variant => &[$(&[$($word),+]),+], )+
                        }
                    }

                    pub(super) fn usage(self) -> &'static [&'static str] {
                        match self {
                            $( $sub::$variant => &[$($usage),*], )+
                        }
                    }
                }
            }
        )+

        impl Command {
            /// Every command, the subcommands of each group together.
            pub const ALL: &'static [Command] = &[
                $( Command::$leaf, )+
                $( $( Command::$group($module::$sub::$variant), )+ )+
            ];

            /// The words that name the command, after `builds`: each way of
            /// naming it.
            pub fn words(self) -> &'static [&'static [&'static str]] {
                match self {
                    $( Command::$leaf => &[$(&[$($leaf_word),+]),+], )+
                    $( Command::$group(sub) => sub.words(), )+
                }
            }

            /// The command's usage lines, each a whole line of help's.
            pub fn usage(self) -> &'static [&'static str] {
                match self {
                    $( Command::$leaf => &[$($leaf_usage),*], )+
                    $( Command::$group(sub) => sub.usage(), )+
                }
            }
        }
    };
}

declare! {
    leaves {
        Help => [["help"], ["-h"], ["--help"]], [
            "builds help workflow",
            "builds help lists",
            "builds help briefs",
            "builds help [workflow|lists|briefs] [SECTION...]",
        ];
        Ack => [["ack"]], [
            "builds ack [--agent ID] [--project P] [--role ROLE]...",
            "builds ack part PART TOKEN [--agent ID] [--project P] [--role ROLE]...",
            "builds ack VERSION TOKEN [--agent ID] [--project P] [--role ROLE]...",
            "builds ack --compacted",
            "builds ack --codex-compacted",
        ];
        RulesOnEdit => [["rules-on-edit"]], ["builds rules-on-edit"];
        Setup => [["setup"]], ["builds setup [--no-prepare] [<checkout>]"];
        Lake => [["lake"]], ["builds lake build [<args>...]"];
        Tool => [["tool"]], ["builds tool <name> [<checkout>]"];
        New => [["new"]], [
            "builds new [<repo>] [-b <branch> | --resume <branch>] [--for <who>] [--project P]",
        ];
        Claim => [["claim"]], ["builds claim <checkout> <who> [--project P]"];
        Note => [["note"]], [
            "builds note <checkout> [--state waiting-on-you|winding-down|held] [<text>...]",
        ];
        Release => [["release"]], ["builds release <checkout>"];
        Retire => [["retire"]], ["builds retire <checkout>"];
        Cargo => [["cargo"]], [
            "builds cargo <subcommand> [<args>...]",
            "builds cargo update (-p <package>... | -w) [<args>...]",
            "builds cargo tree|metadata|locate-project|pkgid|verify-project [<args>...]",
        ];
        BuildApp => [["build-app"]], ["builds build-app <repo> <app> [<ref>]"];
        Lock => [["lock"]], ["builds lock <repo> --lane <app> <command> [<args>...]"];
        Programs => [["programs"]], ["builds programs hold -- <command> [<args>...]"];
        Queue => [["queue"]], [
            "builds queue <repo> [--lane <app>]",
            "builds queue <repo> top <pid>",
        ];
        Takeover => [["takeover"]], ["builds takeover <repo> [--lane <app>] [<pid>]"];
        AppClone => [["app-clone"]], ["builds app-clone <app> <source> <new>"];
        SccacheTmpdir => [["sccache-tmpdir"]], ["builds sccache-tmpdir <pid>"];
        Prune => [["prune"]], ["builds prune [--dry-run] [--idle <minutes> [--include-warm]]"];
        Du => [["du"]], ["builds du <path>..."];
        Status => [["status"]], [
            "builds status",
            "builds status --json [--no-folders]",
        ];
        Health => [["health"]], ["builds health [--json | --stop-junk]"];
        InstallHook => [["install-hook"]], ["builds install-hook <repo>"];
        Waits => [["waits"]], ["builds waits [--project <project>] [--since <YYYY-MM-DD>]"];
        Land => [["land"]], [
            "builds land <worktree> [--project P] [--done N]... [--part N]... [--design N]... \
             [<sha>...]",
        ];
        Brief => [["brief"]], ["builds brief [--project P] PATH..."];
        Install => [["install"]], ["builds install"];
        Version => [["version"]], ["builds version"];
        Next => [["next"]], ["builds next [--for coordinator|design-session] [--project P]"];
    }
    groups {
        Agent(agent::Agent) {
            Start => [["agent", "start"]], ["builds agent start"];
            Run => [["agent", "run"]], [
                "builds agent run --role implementing|planner N [M...] [--brief P] \
                 [--message-file F] [--project P] [--by WHO]",
            ];
            Resume => [["agent", "resume"]], [
                "builds agent resume SESSION [--item N]... [--message-file F] [--project P] [--by WHO]",
            ];
            List => [["agent", "list"]], ["builds agent list [--running] [--project P]"];
            Show => [["agent", "show"]], ["builds agent show SESSION [--project P]"];
        }
        AppBuild(app_build::AppBuild) {
            /// A build of a ref, the process of one of its lanes, or the list
            /// of a repo's steps.
            Build => [["app-build"]], [
                "builds app-build <repo> <ref> [--test <Class>]... [--shots <prefix>]... \
                 [--design-shots] [--all-shots <app>]... [--fresh] [--project P]",
                "builds app-build <repo> <ref> <step>... [--test <app> <Class>]... [--shots <app> \
                 <prefix>]... [--all-shots <app>]... [--fresh] [--project P]",
                "builds app-build <repo> --lane <app> <ref> --built-ref <name> [--project P] \
                 [--needs-only] [--fresh] [<step>...] [--test <lane> <Class>]... [--shots <lane> \
                 <prefix>]... [--all-shots <lane>]...",
                "builds app-build <repo> --list",
            ];
            LandingCoverage => [["app-build", "--landing-coverage"]], [
                "builds app-build --landing-coverage <worktree> <branch> <ref> [<own commit>...]",
            ];
            DropShots => [["app-build", "--drop-shots"]], [
                "builds app-build --drop-shots <repo> <branch> <lane> <prefix> --why TEXT [--by WHO] \
                 [--project P]",
            ];
            RecordHanded => [["app-build", "--record-handed"]], [
                "builds app-build --record-handed <repo> <store> [<project>]",
            ];
        }
        Shots(shots::Shots) {
            Findings => [["shots", "findings"]], [
                "builds shots findings <repo> <ref> <app> <prefix> [--by-cause]",
            ];
            Picture => [["shots", "picture"]], [
                "builds shots picture <repo> <ref> <scene> board|shot [--project P]",
            ];
            CheckProgram => [["shots", "check-program"]], [
                "builds shots check-program <repo> <ref> <app> <prefix> [--checkout <folder>]",
            ];
            /// Every other `shots` command: the shots checker's own.
            Checker => [["shots"]], [
                "builds shots draws --at <rev> --against <rev> [--board <name>]... [--component \
                 <name>]...",
                "builds shots harness mac --into <folder>",
                "builds shots harness android --into <folder>",
                "builds shots \
                 run|scenes|capture|check|lint|runtime|chrome|components|states|version|recheck \
                 [<args>...]",
            ];
        }
        AppCheckout(app_checkout::AppCheckout) {
            Add => [["app-checkout"]], [
                "builds app-checkout <app> <checkout>",
                "builds app-checkout <app> --host <host> <path>",
            ];
            Remove => [["app-checkout", "remove"]], [
                "builds app-checkout remove <app> <checkout>",
                "builds app-checkout remove <app> --host <host> <path>",
            ];
            Reset => [["app-checkout", "reset"]], [
                "builds app-checkout reset <repo> <checkout> [--matching <rev>]",
            ];
        }
        Host(host::Host) {
            Add => [["host", "add"]], ["builds host add <name> <ssh target>"];
            Setup => [["host", "setup"]], [
                "builds host setup <name>",
                "builds host setup local",
            ];
            SetupHere => [["host", "setup-here"]], ["builds host setup-here [<version>...]"];
            SpeedHere => [["host", "speed-here"]], ["builds host speed-here"];
            Band => [["host", "band"]], [
                "builds host band <name> <low> <high>",
                "builds host band <name> clear",
            ];
            List => [["host", "list"]], ["builds host list"];
            Cache => [["host", "cache"]], ["builds host cache"];
            Remove => [["host", "remove"]], ["builds host remove <name>"];
            ClearXcode => [["host", "clear-xcode"]], ["builds host clear-xcode <name>"];
        }
        Remote(remote::Remote) {
            Run => [["remote", "run"]], [
                "builds remote run <repo> <ref> [--host <name>] -- <command> [<args>...]",
                "builds remote run <repo> --staged [--host <name>] -- <command> [<args>...]",
                "builds remote run <repo> --working [--host <name>] -- <command> [<args>...]",
            ];
            Step => [["remote", "step"]], [
                "builds remote step <host> <worktree> -- <command> [<args>...]",
            ];
        }
        Units(units::Units) {
            Prune => [["units", "prune"]], [
                "builds units prune <days> <incremental hours> <dry 0|1> <folder>...",
            ];
            GitCheckouts => [["units", "git-checkouts"]], [
                "builds units git-checkouts <cargo home> <dry 0|1> <Cargo.lock>...",
            ];
            AfterBuild => [["units", "after-build"]], ["builds units after-build <checkout>"];
        }
        Dashboard(dashboard::Dashboard) {
            Stage => [["dashboard", "stage"]], [
                "builds dashboard stage --fixtures <folder> <shot>",
            ];
            Serve => [["dashboard"]], ["builds dashboard [--port <port>] [--listen <address>]"];
        }
        Checks(checks::Checks) {
            Run => [["checks", "run"]], [
                "builds checks run [<worktree>]",
                "builds checks run --since <rev> [<worktree>]",
            ];
            Whole => [["checks", "whole"]], ["builds checks whole [<worktree>]"];
            Stats => [["checks", "stats"]], [
                "builds checks stats [--project <project>] [--repo <repo>] [--since <YYYY-MM-DD>] \
                 [--change <rev>]",
            ];
            List => [["checks", "list"]], ["builds checks list [<worktree>]"];
            Tests => [["checks", "tests"]], ["builds checks tests --since <rev> [--run]"];
        }
        Review(review::Review) {
            /// `review <worktree>`, and `review run <worktree>`.
            Run => [["review"], ["review", "run"]], [
                "builds review [run] <worktree> [<base>] --kind code|mechanical [--checked-by \
                 TEXT] [--landing <key>] [--item N]... [--head SHA] [--rules-root DIR] \
                 [--reviewer PROGRAM] [--by WHO]",
            ];
            Copy => [["review", "copy"]], [
                "builds review copy <worktree> [<base>] [--rules-root DIR] [--reviewer PROGRAM] [--by WHO]",
            ];
            Settle => [["review", "settle"]], [
                "builds review settle <finding> confirmed|dropped --why <text> [--by WHO]",
                "builds review settle <finding> --again --why <text> [--by WHO]",
                "builds review settle <finding> allowed --by-person --why <text> [--by WHO]",
            ];
            Wait => [["review", "wait"]], ["builds review wait <landing>"];
            Dispute => [["review", "dispute"]], ["builds review dispute <finding> --why <text> [--by WHO]"];
            Keep => [["review", "keep"]], [
                "builds review keep <finding> --why <text> [--by WHO]",
                "builds review keep <finding> --again --why <text> [--by WHO]",
            ];
            Show => [["review", "show"]], ["builds review show <landing>"];
            Key => [["review", "key"]], ["builds review key <worktree> --project P"];
            CheckLanding => [["review", "check-landing"]], [
                "builds review check-landing <worktree> --project P <sha>... [--landing KEY] \
                 [--squashed] [--rules-root DIR]",
            ];
            Landed => [["review", "landed"]], [
                "builds review landed <worktree> --project P <sha>... [--landing KEY] \
                 [--more-to-land] [--rules-root DIR] [--by WHO]",
            ];
            Groups => [["review", "groups"]], [
                "builds review groups [--rules-root DIR] [--project P --repo R --changed PATH...]",
            ];
            Replay => [["review", "replay"]], [
                "builds review replay [--case NAME]... [--source REPO] [--model <model>] \
                 [--rules-root DIR] [--compare] [--reviewer PROGRAM] [--by WHO]",
                "builds review replay --landing N... --repo-dir DIR [--group NAME]... [--model \
                 <model>] [--rules-root DIR] [--reviewer PROGRAM] [--by WHO]",
            ];
            Audit => [["review", "audit"]], [
                "builds review audit <landing> --repo-dir DIR [--rules-root DIR] [--reviewer \
                 PROGRAM] [--seed N] [--by WHO]",
            ];
            Misses => [["review", "misses"]], [
                "builds review misses",
                "builds review misses --check [--model <model>] [--rules-root DIR] [--reviewer \
                 PROGRAM]",
            ];
            Rates => [["review", "rates"]], ["builds review rates [--rules-root DIR]"];
            Scores => [["review", "scores"]], ["builds review scores"];
            CasePropose => [["review", "case", "propose"]], [
                "builds review case propose --repo-dir DIR --rules-root DIR [--per-group N] \
                 [--model <model>] [--reviewer PROGRAM]",
            ];
            CaseAdd => [["review", "case", "add"]], [
                "builds review case add <landing> <finding> --repo-dir DIR --rules-root DIR \
                 [--name NAME]",
                "builds review case add (--rule SLUG --file PATH | --clean-for SLUG) --commits \
                 BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR",
            ];
        }
        Judge(judge::Judge) {
            Listen => [["judge", "listen"]], ["builds judge listen --project P [--by WHO]"];
        }
        Question(question::Question) {
            Ask => [["question", "ask"]], [
                "builds question ask --title T --kind question|your-step --effect E \
                 --details-file F (--blocks N[,N] | --blocks-nothing WHY) [--from-item N | \
                 --origin N] [--first WHY] [--plan PATH --plan-number Qn] [--tag T]... [--by WHO]",
            ];
            Answer => [["question", "answer"]], ["builds question answer N --request F [--by WHO]"];
            Settle => [["question", "settle"]], ["builds question settle N --request F [--by WHO]"];
            Done => [["question", "done"]], ["builds question done N --request F [--by WHO]"];
            Drop => [["question", "drop"]], ["builds question drop N --why WHY [--by WHO]"];
            Place => [["question", "place"]], [
                "builds question place N (first|last|auto | --above M | --below M) --why WHY \
                 [--by-person] [--by WHO]",
            ];
            Edit => [["question", "edit"]], [
                "builds question edit N [--title T] [--details-file F] [--effect E] \
                 [--blocks-nothing WHY] [--origin N] [--tag T]... [--untag T]... [--by WHO]",
            ];
            Show => [["question", "show"]], ["builds question show N"];
            List => [["question", "list"]], ["builds question list [--tag T]"];
        }
        Item(item::Item) {
            Add => [["item", "add"]], [
                "builds item add --title T --area A --details-file F [--for design-session] \
                 [--wait REF [--until ENDER]]... [--brief P] [--backlog WHY] [--priority \
                 first|last --why WHY] [--origin N] [--tag T]... [--by WHO]",
            ];
            Edit => [["item", "edit"]], [
                "builds item edit N [--title T] [--area A] [--details-file F] [--brief P] [--for \
                 F] [--origin N] [--tag T]... [--untag T]... [--by WHO]",
            ];
            Note => [["item", "note"]], ["builds item note N TEXT... [--by WHO]"];
            Changes => [["item", "changes"]], ["builds item changes N [--by WHO]"];
            Wait => [["item", "wait"]], [
                "builds item wait N (--on REF [--until ENDER] | --off REF [--blocks-nothing WHY]) \
                 [--by WHO]",
            ];
            Backlog => [["item", "backlog"]], ["builds item backlog N [M...] --why WHY [--by WHO]"];
            Start => [["item", "start"]], ["builds item start N [M...] --said WORDS [--by WHO]"];
            Take => [["item", "take"]], [
                "builds item take N [M...] [--worktree W --branch B [--worktree W2 --branch B2] | \
                 --repo R --branch B [--repo R2 --branch B2]] --brief P [--as design-session] \
                 [--tier implementation|strongest] [--held WHY] [--rules-root DIR] [--reviewer \
                 PROGRAM] [--by WHO]",
            ];
            CheckBrief => [["item", "check-brief"]], [
                "builds item check-brief --worktree W [--worktree W2] --brief P [--rules-root \
                 DIR] [--reviewer PROGRAM]",
            ];
            Handoff => [["item", "handoff"]], ["builds item handoff N --brief P [--by WHO]"];
            Stop => [["item", "stop"]], ["builds item stop N --why WHY [--by WHO]"];
            Finished => [["item", "finished"]], ["builds item finished N --made WHAT [--by WHO]"];
            Drop => [["item", "drop"]], ["builds item drop N --why WHY [--by WHO]"];
            Place => [["item", "place"]], ["builds item place N first|last|none --why WHY [--by WHO]"];
            CheckLanding => [["item", "check-landing"]], [
                "builds item check-landing DIR [--done N]... [--part N]... SHA...",
            ];
            Landed => [["item", "landed"]], [
                "builds item landed DIR [--done N]... [--part N]... SHA... [--by WHO]",
            ];
            Show => [["item", "show"]], ["builds item show N"];
            List => [["item", "list"]], [
                "builds item list [--running] [--ready] [--backlog] [--area A] [--for F] [--tag T]",
            ];
            Times => [["item", "times"]], ["builds item times [N...]"];
        }
        Area(area::Area) {
            Add => [["area", "add"]], ["builds area add NAME [--after AREA] [--by WHO]"];
            Rename => [["area", "rename"]], ["builds area rename OLD NEW [--by WHO]"];
            Move => [["area", "move"]], ["builds area move NAME --after AREA [--by WHO]"];
            Remove => [["area", "remove"]], ["builds area remove NAME [--by WHO]"];
        }
        Decision(decision::Decision) {
            Add => [["decision", "add"]], ["builds decision add --request FILE [--by WHO]"];
            Edit => [["decision", "edit"]], [
                "builds decision edit N [--title T] [--body-file FILE|-] [--by WHO]",
            ];
            Find => [["decision", "find"]], ["builds decision find WORD..."];
            Show => [["decision", "show"]], ["builds decision show N"];
            List => [["decision", "list"]], [
                "builds decision list [--plan PATH] [--since YYYY-MM-DD]",
            ];
        }
        Design(design::Design) {
            Places => [["design", "places"]], ["builds design places DIR REV..."];
            Only => [["design", "only"]], ["builds design only DIR REV..."];
            Record => [["design", "record"]], ["builds design record --request FILE [--by WHO]"];
            Amend => [["design", "amend"]], ["builds design amend N --request FILE [--by WHO]"];
            Approve => [["design", "approve"]], ["builds design approve N [--by WHO]"];
            Check => [["design", "check"]], ["builds design check N[,N...] DIR REV..."];
            Landed => [["design", "landed"]], ["builds design landed N DIR SHA... [--by WHO]"];
            Title => [["design", "title"]], ["builds design title N"];
            Drop => [["design", "drop"]], ["builds design drop N --why WHY [--by WHO]"];
            Show => [["design", "show"]], ["builds design show N"];
            List => [["design", "list"]], ["builds design list"];
            Render => [["design", "render"]], [
                "builds design render DIR BOARD [--theme light|dark] [--now | --at REV]",
            ];
            TokensCss => [["design", "tokens-css"]], ["builds design tokens-css DIR"];
            PublishFind => [["design", "publish-find"]], [
                "builds design publish-find DIR --artifacts FILE [--at REV]",
            ];
            PublishFiles => [["design", "publish-files"]], [
                "builds design publish-files DIR OUT --artifacts FILE (--index FILE | --no-index) \
                 [--at REV]",
            ];
            PublishLast => [["design", "publish-last"]], [
                "builds design publish-last FOLDER --listing FILE [(--index FILE | --no-index) \
                 --by WHO [--uploaded FILE]]",
            ];
            Listen => [["design", "listen"]], ["builds design listen --project P [--by WHO]"];
            PublishTake => [["design", "publish", "take"]], [
                "builds design publish take N [--project P] [--by WHO]",
            ];
            PublishDone => [["design", "publish", "done"]], [
                "builds design publish done N --link TITLE=URL... [--project P] [--by WHO]",
            ];
            PublishFailed => [["design", "publish", "failed"]], [
                "builds design publish failed N --why WHY [--project P] [--by WHO]",
            ];
        }
        Lists(lists::Lists) {
            Init => [["lists", "init"]], [
                "builds lists init PROJECT --repos R,R [--canvas yes|no] [--design-places \
                 R:GLOB,...] [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--by WHO]",
            ];
            DesignGlobs => [["lists", "design-globs"]], [
                "builds lists design-globs PROJECT [--canvas yes|no] [--design-places R:GLOB,...] \
                 [--design-sources R:GLOB,...] [--spec-files R:GLOB,...] [--by WHO]",
            ];
            AddRepo => [["lists", "add-repo"]], ["builds lists add-repo PROJECT REPO [--by WHO]"];
            Show => [["lists", "show"]], ["builds lists show [--json | --tag T] [--project P]"];
            ProjectOf => [["lists", "project-of"]], ["builds lists project-of DIR"];
        }
    }
}

/// A command line of builds read into its command.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed<'a, S> {
    pub command: Command,
    /// The words that name the command.
    pub named: &'a [S],
    /// The words after them.
    pub rest: &'a [S],
}

impl Command {
    /// The command that <words>, a command line of builds after the program
    /// and its `--agent` flag, names: the command whose words the most of
    /// <words> start with. Fails, naming the commands or the forms of the
    /// command, when none does.
    pub fn parse<S: AsRef<str>>(words: &[S]) -> Result<Parsed<'_, S>> {
        if let Some(parsed) = Command::of(words) {
            return Ok(parsed);
        }
        let Some(first) = words.first().map(AsRef::as_ref) else {
            fail!("usage: builds <command> [<args>...] (`builds help`)");
        };
        let forms = Command::forms(first);
        if forms.is_empty() {
            fail!(
                "builds has no command {first}. Its commands are {}.",
                and_list(&Command::names())
            );
        }
        let what = match words.get(1).map(AsRef::as_ref) {
            Some(word) if !word.starts_with('-') => format!("has no command {word}"),
            _ => "needs a command".to_string(),
        };
        fail!(
            "builds {first} {what}. Its forms are:\n  {}",
            forms.join("\n  ")
        )
    }

    /// The usage lines of every command whose first word is <first>.
    fn forms(first: &str) -> Vec<&'static str> {
        Command::ALL
            .iter()
            .filter(|command| command.words().iter().any(|named| named[0] == first))
            .flat_map(|command| command.usage().iter().copied())
            .collect()
    }

    /// What a refusal of the command says of its use: `usage:` and each of
    /// its usage lines.
    pub fn usage_text(self) -> String {
        usage_text(self.usage())
    }

    /// `usage_text` for every command whose first word is <first>: what the
    /// refusals of code that several of them share say.
    pub fn group_usage_text(first: &str) -> String {
        usage_text(&Command::forms(first))
    }

    /// `parse`, for a command line another process runs: None when it names
    /// no command.
    pub fn of<S: AsRef<str>>(words: &[S]) -> Option<Parsed<'_, S>> {
        let mut best: Option<Parsed<'_, S>> = None;
        for command in Command::ALL {
            for named in command.words() {
                let starts = named.len() <= words.len()
                    && named
                        .iter()
                        .zip(words)
                        .all(|(name, word)| *name == word.as_ref());
                if starts
                    && best
                        .as_ref()
                        .is_none_or(|best| best.named.len() < named.len())
                {
                    let (named, rest) = words.split_at(named.len());
                    best = Some(Parsed {
                        command: *command,
                        named,
                        rest,
                    });
                }
            }
        }
        best
    }

    /// The first word of each command, once each, in help's order.
    pub fn names() -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for command in Command::ALL {
            let name = command.words()[0][0];
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

/// `builds <group> <words>` read into its command and its arguments, for a
/// test that runs a command of <group> by its words.
#[cfg(test)]
pub fn parse_line(group: &str, words: &[String]) -> Result<(Command, crate::args::Args)> {
    let line: Vec<String> = std::iter::once(group.to_string())
        .chain(words.iter().cloned())
        .collect();
    let parsed = Command::parse(&line)?;
    Ok((
        parsed.command,
        crate::args::Args::of(parsed.command, parsed.rest)?,
    ))
}

/// `usage:` and each of <lines>, one under the other.
fn usage_text(lines: &[&str]) -> String {
    format!("usage: {}", lines.join("\n       "))
}

/// <names> as a sentence lists them: "a, b and c".
pub fn and_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
