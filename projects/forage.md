# forage

Forage is a language for scraping and the runtime that runs it. A `.forage`
recipe describes how to fetch structured records from a site or API, and the
runtime runs recipes against an HTTP engine or a real browser engine (a
wry-backed WebView). Forage comes with a CLI, a language server and Forage
Studio, an app for writing recipes interactively.

## Shapes change in one commit, with no compatibility code

forage follows
`rules/change-a-shape-in-one-commit-with-no-compatibility-code.md`. In forage,
its shapes include the schema, the AST, the IR, the binary format, the capture
and replay fixtures, the hub-api KV entries and the language server's wire
messages.
