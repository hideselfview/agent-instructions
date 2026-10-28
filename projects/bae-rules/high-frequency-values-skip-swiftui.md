---
digest: A value that changes many times a second (playback position, transfer bytes, rates, levels) reaches the screen by setting an AppKit/UIKit view or layer directly from its stream, never through SwiftUI observation.
paths:
  - '**/*.swift'
  - '**/*.kt'
blocking: false
---

## High-frequency values skip SwiftUI

Some values change many times a second: playback position, the bytes a file
upload or download has moved, transfer rates, meter levels. Core sends them on
keyed streams, not in tables. On screen they are drawn by a view the stream sets
directly, not by SwiftUI state.

Routing such a value through an `@Observable` store or `@State` invalidates
SwiftUI views on every change: every body that reads it re-runs, diffs and lays
out, on the main thread, for each tick. Under a fast upload that is hundreds of
updates a second across every visible row.

**On macOS and iOS:** the stream's subscriber holds the AppKit/UIKit view (or
`CALayer`) and sets its value: a bar's fraction, a label's text, a ring's
`strokeEnd`. SwiftUI places the view once through a representable and only
passes it the stream; `updateNSView` handles configuration (colors, the key the
stream is for), never the value. `SeekBarNSView`, `ProgressStripNSView` and
`ProgressTrackNSView` are the pattern.

**On Android:** the same split. A Compose value read during composition
recomposes on every tick; read it in a draw or layout lambda
(`Modifier.drawBehind`, `graphicsLayer`) or set a View directly.

**Which values are "high-frequency":** anything core sends faster than about
once a second while it is active. Counts that change when a state changes
(queued, stored, pinned) are state from tables and go through SwiftUI like any
other row data.

**Smell:** a store property updated from a progress or position stream and read
in a SwiftUI `body`; a `Text("\(rate)")` fed from a throughput stream.
