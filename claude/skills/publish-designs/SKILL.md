---
name: publish-designs
description: Publish a project's designs to claude.ai, every canvas of its boards and its Design System artifact, all from one commit, with `builds design publish-files` and the Artifact tool. Use after a landing that changes the boards or the design system of a project that publishes a canvas (bae), or when asked to publish the canvas, the boards or the design system.
---

# Publish every canvas and the design system from one commit

This skill is for a Claude session that publishes a project's designs after a
landing. The designs are the boards (`design/mocks`) and the design system
(`design/system`) of the project's repo. The repo publishes them to several
claude.ai artifacts: one canvas or more, which show the boards, and one Design
System artifact. `design/mocks/canvases.toml` in the repo names each of them.

Every publish sends every artifact from the same commit, so the published copies
never disagree. builds writes the files and only a session can call the Artifact
tool, so the work splits in two: `builds design publish-files` writes every
artifact's files and its publish calls from one commit, and the session makes
the calls in order. `builds help lists` (design publish-files and publish-last)
says what each command writes.

The repo's files are the source. An edit someone made on an artifact's page on
claude.ai is an accident: the repo's files replace it, and you merge nothing
from it.

## Write the files

Run `builds design publish-files <worktree> <out>`, where `<worktree>` is any
checkout of the repo and `<out>` is a new folder in your scratchpad. The
Artifact tool takes a `root` only under the working directory or the scratchpad.
Leave out `--at`, so that the files come from origin/main, which holds what
landed.

The command prints the commit, then each artifact in the order you publish them:
the canvases, then the design system. For each artifact it prints the folder and
the call files (`call-1.json` and on). It ends with the sha256 that the repo
records for the canvas runtime. When the command refuses, publish nothing and
report its error.

## Publish each artifact

Do these steps for each artifact, in the printed order:

1. Read the artifact with the Artifact tool (`read` with its url). The tool
   refuses a publish to an artifact that this conversation has not read.
2. Make each call, in order: Read the call file and pass its `url`, `root`,
   `file_path` and `files` to the Artifact tool's `publish` as they are. Add no
   other field, such as `capabilities`, `contract` or `icon`.
3. List the artifact's files (`list` with its url and scope `files`), through
   every page of the listing.
4. For the design system only, read its index (`read` with its url and path
   `project/design-system.json`). The result says where the tool saved the file.
5. Run `builds design publish-last <folder> --listing -` with every line of the
   listing on standard input, in a quoted heredoc (`<<'EOF'`). For the design
   system, add `--index <the saved index> --by "<the person's name>"`.
6. Make each last call that publish-last prints, in order, as in step 2. A
   canvas with no file to remove has no last call.

When the Artifact tool refuses a call because someone saved the artifact since
you read it, the refusal counts as a read. Make the same call again, since the
repo's files replace every edit made on the page. When the tool refuses a last
call, list the files again, run publish-last again, and make its new calls.

## Check the canvas runtime

After the last artifact, read the canvas runtime from each canvas (`read` with
the canvas's url and the path that publish-files printed, such as
`artifact-type/dc-runtime.js`). The result gives the file's sha256. When a
canvas's sha256 differs from the one publish-files printed, the canvas runtime
changed and the shot check still draws boards with the old one: add a queue item
that updates `design/mocks/runtime.toml` and the shot check's copy of the
runtime (`scripts/boards/src/runtime.rs` in agent-instructions says how).

## Report

Report the commit, each artifact's url, the files the last calls removed, and
the result of the runtime check. When you could not finish every artifact, say
which artifacts hold the new commit and which still hold an older one. The next
session runs this skill again from the start, with a new folder.
