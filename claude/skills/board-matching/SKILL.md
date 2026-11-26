---
name: board-matching
description: Run the rounds of work that make every shot of an app match its board, on every platform, until a full measurement shows no findings or only ones a board decision settled. A coordinator measures every shot, fixes the causes many scenes share first, then runs agents in parallel on a few large chunks of components and screens, each of which clears every finding in its chunk before it reports. Use when asked to make an app's shots match its boards, for board matching, or for shot findings across many screens.
---

# Board matching

A coordinator session loads this skill in a project that keeps boards and shots
through builds. A board is a picture of one app screen that the repo holds, and
a shot is a screenshot of the real app view that the shots checker compares with
its board (`builds help workflow`, DESIGN TERMS). Each difference the checker
reports is a finding. A lane is one platform's app builds, such as mac or
android (`builds help`, APP BUILDS).

The goal is that a full measurement on every lane shows no findings, or only
ones a board decision settled. The coordinator reaches it in as few rounds as it
can. Each round measures every shot, fixes the causes that many scenes share,
measures again, and then splits the rest into a few large chunks, such as one
platform's components or a group of its screens. One agent owns each chunk, the
chunks run in parallel, and each agent clears every finding in its chunk before
it reports (Chunks, below). The coordinator runs the rounds and dispatches the
fixes; its agents implement, review and land them as for any item
(`builds help workflow`, WHAT AN IMPLEMENTING AGENT DOES). This skill states the
method, and `builds help` gives each command's details.

## Plan and item

Before the first round, write a software plan for the work in the repo's
`plans/board-matching.md` (`builds help workflow`, PLAN, BRIEF, EXECUTE). The
plan states the goal, the round it is in, what each running agent fixes, and a
**Next** line: the step that starts next and the condition it waits for, such as
"when the component agents land, measure both lanes again". Open one item that
names the plan, and give every item a round queues that item as its origin
(`--origin N`, `builds help lists`).

While a round runs, start no other work unless that work blocks one of the
round's landings. Hold every other item in the queue until the round ends, and
put new questions on the decision stack for after.

After each landing, update the plan's current round and its **Next** line. On
each health check (`builds help workflow`, HEALTH CHECK), read the **Next** line
and start that step as soon as its condition holds.

## Rounds

A round is one cycle of these steps:

1. Measure every lane.
2. Find the shared causes, and the shots that draw something other than their
   board.
3. Fix the shared causes, the shots' staging and the previews.
4. Measure every lane again.
5. Split the findings left into chunks and run one agent per chunk, all at once:
   the components, and the screens in a few groups (Chunks, Components and
   Screens, below).
6. Measure every lane again once the chunks land.

Start another round only for the findings a measurement still shows. Repeat
rounds until a measurement meets the goal. Then close the plan's item, and
delete the plan and any line of the project file that names it.

## Measurements

A measurement is a full run of one lane's shots on a commit of origin/main, and
the coordinator runs one for each lane (`builds help workflow BUILDS`;
`builds help APP BUILDS` gives the command and what the run compares with). Read
its findings, not its exit status, and rank them by the design-system component
that draws each one
(`builds shots findings <repo> <commit> <lane> <lane>. --by-cause`). The command
also prints where the run's report is: its findings.json holds every finding as
a record, with its scene, kind, board element, component and numbers.

A fix to how the checker compares needs no new shots: once the fix is installed,
measure again by reading the findings, which builds checks again with the
installed checker (`builds help APP BUILDS`).

## Shared causes

A shared cause is one fault that makes findings in many scenes or components.
Shared-cause agents run before component agents, because a shared cause changes
the files of many components, and a component agent running at the same time
would change the same files.

Group the findings three ways, and count the scenes and components each group
spans:

- by colour pair, the exact board colour and shot colour, which `--by-cause`
  prints;
- by offset, how far each element moved, which `--by-cause` also prints;
- by board element, for every other kind, from findings.json.

A group that spans many scenes is a shared cause. Shared causes include these
kinds, each with an example:

- A wrong token: the app draws secondary text in the system's colour, where the
  design system has its own token.
- A component every screen uses: every sheet and popover shares one sheet face,
  so a wrong face makes findings on every screen that opens a sheet.
- A fault in the checker: the checker compared a non-text element by its
  majority colour, and it took a text's colour from the text's whole box, not
  from its glyphs.
- A fault in what the shot tests report to the checker: the app's shot test code
  clipped each element's box to the screen before it wrote the element list. The
  fix goes in the app's test code, not in the checker.

Queue one item per shared cause. Group the items into as few agents as keeps
each agent's files apart from every other's, and run those agents at the same
time. A fault in the checker is builds work: its agent checks the fix with
`builds shots check-program` over the measurement's cached run.

## Staging and previews

A count of findings means something only when each shot draws what its board
draws. Look for two faults while you find the shared causes, and queue their
fixes in the same step.

**Staging.** A scene's staging is the data and state the shot test gives the app
before it takes the shot. Fix staging that leaves a scene without the board's
data, or that draws only part of the page the board draws, so that the next
measurement counts those screens. A staging fix can raise a screen's findings,
because more of the page now draws and its components' faults show. The staging
agent then lands the staging and drops that shots prefix with
`builds app-build --drop-shots`, with a reason that names the component items
the new findings belong to.

**Previews.** A preview is a picture of one component on its own, one row for
each of its configurations. A component's shots compare the app with its preview
rows, so a preview that draws a component too narrow or collapsed makes every
one of those shots wrong, such as a preview that squeezes a title bar under its
tabs, or one that gives a grid no width. Read the preview of each component near
the top of the ranking. A preview fault goes to the design session
(`builds item add --for design-session`).

## Chunks

A chunk is a large share of one round's findings that one agent owns, such as
every component of a lane, or a group of a lane's screens. Cut a round into as
few chunks as keeps each agent's files apart from every other's, and run them at
the same time. A chunk of a few findings wastes a review, an app build and a
landing on work that another chunk's agent could have done in the same branch.

The agent clears every finding in its chunk before it reports. Each finding ends
in one of two ways:

- fixed and landed;
- traced to a cause outside the app, such as a board, a preview or the checker,
  and reported with its numbers on both sides, so that the coordinator can send
  it on (Differences between board and app, below).

The agent defers a finding only for a reason it names: a fix that another
running agent lands changes the finding, or the finding needs a board decision.
It lands its work in parts as it goes (`--part N`), with one commit per cause
and one review and app build per part, and it reports once nothing in the chunk
is left. When an agent reports with findings it has not worked, the coordinator
sends it straight back with the list.

## Components

The components chunk takes every component in the ranking, from the top. Its
agent fixes what is wrong inside each component once, never once per screen: its
size, its colours, its missing parts, and every finding in the component's own
shots. A finding that an element moved belongs to a screens chunk, which fixes
the layout that places the component.

No two running branches change one component's files. An item whose component
another running branch changes waits on that branch's item (`builds item wait`).

Each part of the chunk goes on one branch: the agent reviews it, runs one app
build with the shots of the components it changed last (`builds help workflow`,
WHAT AN IMPLEMENTING AGENT DOES) and lands it. Each shots run compares with the
run of its own merge base, so a branch behind origin/main needs no rebase to
compare (`builds help APP BUILDS`). An agent rebases when `builds land` refuses
because its cherry-pick conflicts, or when the coordinator sends it back after a
landing that changed shared code under it (`builds help workflow LANDING`).

## Screens

Each screens chunk fixes, screen by screen, the findings that are no component's
own:

- the layout that places each component on the screen, which fixes the moves the
  components chunk left, such as a shot window narrower than its board, which
  moves every tile and row in it;
- the state the screen shows;
- each difference between board and app that needs a board decision (below).

## Differences between board and app

When the board and the app disagree on what the screen should show, such as a
scrim behind a sheet, or an accessibility label the app has and the board lacks,
the agent that finds it reports both sides to the coordinator instead of
choosing. The coordinator sends each one on:

- A board that should draw what the app already draws goes to the design session
  as a fix (`builds help workflow`, DESIGN CHANGES).
- A real choice between the two goes to the design session when the project's
  board rule lets the design session make it, as bae's does
  (`projects/bae-rules/screen-content/the-design-session-settles-board-and-app-differences.md`),
  and otherwise on the decision stack for the person (`builds question ask`).
