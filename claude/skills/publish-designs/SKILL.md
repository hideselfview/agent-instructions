---
name: publish-designs
description: Publish a project's designs to the claude.ai account Claude Code is signed in to, every canvas of its boards and its Design System artifact with its uploads, all from one commit, with `builds design publish-files` and the Artifact tool. Use after a landing that changes the boards or the design system of a project that publishes a canvas (bae), when a "Publish request #N" message from the dashboard arrives, or when asked to publish the canvas, the boards or the design system.
---

# Publish every canvas and the design system from one commit

This skill is for a Claude session that publishes a project's designs after a
landing. The designs are the boards (`design/mocks`) and the design system
(`design/system`) of the project's repo. The repo publishes them to several
claude.ai artifacts: one canvas or more, which show the boards, and one Design
System artifact, which also keeps the design system's pictures as uploads that
its index names. `design/mocks/canvases.toml` in the repo names each artifact by
its title. The artifacts are the ones on whichever claude.ai account Claude Code
is signed in to: builds finds each title among the account's artifacts, so a
change of account needs no edit in the repo.

Every publish sends every artifact from the same commit, so the published copies
never disagree. builds writes the files and only a session can call the Artifact
tool, so the work splits in two: `builds design publish-files` writes every
artifact's files, uploads and calls from one commit, and the session makes the
calls in order. Each call file holds the arguments of one Artifact tool call,
its `action` among them: pass every field as it is, and add none.
`builds help lists` (design publish-find, publish-files and publish-last) says
what each command writes.

The repo's files are the source. An edit someone made on an artifact's page on
claude.ai is an accident: the repo's files replace it, and you merge nothing
from it.

## Find the artifacts

1. List the account's canvases: `list` with `type: "Design"` and `limit: 200`,
   and again with each `page_token` the listing gives, until it gives none. List
   the account's design systems the same way, with `type: "Design System"`.
2. Write every listing, as the tool printed it, into one file in a new folder of
   your scratchpad, such as `<scratch>/<branch>/artifacts.txt`.
3. Run `builds design publish-find <worktree> --artifacts <the file>`, where
   `<worktree>` is any checkout of the repo. It prints the url of each canvas
   and of the design system.

When publish-find says the account has no artifact with a title, create each one
it names: `publish` with the `type_url` of the type it names, the title it gives
as `title`, and no files. A new artifact stays empty until the publish fills it.
Then list the account's artifacts again, write the file again and run
publish-find again. When publish-find says that two of the person's artifacts
share a title, stop and ask the person which one to keep: publish nothing.

## A publish request from the dashboard

The person can ask for a publish on the dashboard's Designs page. builds then
records a publish request for one commit and sends the design session that
listens for the project (`builds design listen --project P`, which a design
session runs when it starts) a message: "Publish request #N of project P:
publish the designs at commit C." For such a request:

1. Take it first: `builds design publish take N --project P`. The dashboard then
   shows that the design session took it.
2. Follow the rest of this skill with `--at C` on `publish-files`, so that the
   files come from the commit the person picked.
3. After the last artifact's last calls and the runtime check (Check the canvas
   runtime), end the request with one `--link` for each artifact, the title and
   url that `publish-find` printed:
   `builds design publish done N --link "<title>=<url>" ... --project P`. The
   dashboard then shows the commit as published, with the links.
4. When the publish stops before that, end the request with what stopped it:
   `builds design publish failed N --why "<what stopped it>" --project P`.

## Write the files

1. Read the design system's index: `read` with the design system's url and path
   `project/design-system.json`. The result says where the tool saved the file.
   When the tool says the artifact has no such file, as a design system you just
   created has none, the design system has no index yet.
2. Run
   `builds design publish-files <worktree> <out> --artifacts <the file> --index <the saved index>`,
   or with `--no-index` in place of `--index <the saved index>` when the design
   system has no index yet. `<out>` is a new folder in your scratchpad. The
   Artifact tool takes a `root` only under the working directory or the
   scratchpad. Leave out `--at`, so that the files come from origin/main, which
   holds what landed, unless a publish request names the commit.

The command prints the commit, then each artifact in the order you publish them:
the canvases, then the design system. For each artifact it prints its title, its
url, the folder and the call files (`call-1.json` and on), and for the design
system the upload files (`upload-1.json` and on) with the path each one uploads.
builds uploads only the assets whose bytes the index records no upload of. The
command ends with the sha256 that the repo records for the canvas runtime. When
the command refuses because the account lacks a title, go back to Find the
artifacts. When it refuses for another reason, publish nothing and report its
error.

## Publish each artifact

Do these steps for each artifact, in the printed order:

1. Read the artifact with the Artifact tool (`read` with its url). The tool
   refuses a publish to an artifact that this conversation has not read.
2. Make each call, in order: Read the call file and pass its fields to the
   Artifact tool as they are.
3. For the design system only, make each upload call, then write the url that
   the tool gave each upload into `<folder>/uploaded.json`, a JSON object keyed
   by the path that publish-files printed for the upload:
   `{"project/assets/Logos/bae-word.svg": "/_blob/<id>"}`.
4. List the artifact's files (`list` with its url and scope `files`), through
   every page of the listing.
5. For the design system only, read its index again, as in Write the files.
6. Run `builds design publish-last <folder> --listing -` with every line of the
   listing on standard input, in a quoted heredoc (`<<'EOF'`). For the design
   system, add `--index <the index saved in step 5> --by "<the person's name>"`
   (`--no-index` in place of `--index` when it still has no index) and, when it
   made uploads, `--uploaded <folder>/uploaded.json`.
7. Make each last call that publish-last prints, in order, as in step 2: first
   the `last-` calls, then the design system's `delete-` calls, which delete the
   uploads its index no longer names. A canvas with no file to remove has no
   last call.

When the Artifact tool refuses a call because someone saved the artifact since
you read it, the refusal counts as a read. Make the same call again, since the
repo's files replace every edit made on the page. When the tool refuses a last
call, list the files again, run publish-last again, and make its new calls. When
publish-last refuses because an asset has no upload of its bytes, someone
changed the index since you read it: start again from Write the files, with a
new folder.

## Check the canvas runtime

After the last artifact, read the canvas runtime from each canvas (`read` with
the canvas's url and the path that publish-files printed, such as
`artifact-type/dc-runtime.js`). The result gives the file's sha256. When a
canvas's sha256 differs from the one publish-files printed, the canvas runtime
changed and the shot check still draws boards with the old one: add a queue item
that updates `design/mocks/runtime.toml` and the shot check's copy of the
runtime (`scripts/boards/src/runtime.rs` in agent-instructions says how).

## Report

Report the commit, each artifact's url, the artifacts you created, the files the
last calls removed, the assets you uploaded and the uploads you deleted, and the
result of the runtime check. When you could not finish every artifact, say which
artifacts hold the new commit and which still hold an older one, and end a
publish request that you did not finish with
`builds design publish failed N --why`. The next session runs this skill again
from the start, with a new folder.
