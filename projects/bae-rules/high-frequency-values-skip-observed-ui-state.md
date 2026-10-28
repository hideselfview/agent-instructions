---
digest: In every bae app, current and future, a value that changes many times a second (playback position, transfer bytes, rates, levels) reaches the screen by setting a native view or layer directly from its stream, never through the UI framework's observed state (SwiftUI, Compose, Avalonia bindings).
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.axaml'
blocking: false
---

## High-frequency values skip the UI framework's observed state

Some values change many times a second: playback position, the bytes a file
upload or download has moved, transfer rates, meter levels. Core sends them on
keyed streams, not in tables. In every bae app, the ones that exist and the ones
still to come, they are drawn by a view the stream sets directly, not by the UI
framework's observed state.

Routing such a value through observed state (SwiftUI `@Observable`/`@State`,
Compose state read in composition, an Avalonia view-model property behind a
binding) re-runs the framework's update on every change: every view that reads
it re-evaluates, diffs and lays out, on the main thread, for each tick. Under a
fast upload that is hundreds of updates a second across every visible row.

Each app has two halves: the framework places the view once and hands it the
stream's key; the view's own subscriber sets the value. A new app follows the
same split from its first progress bar.

**On macOS and iOS:** the stream's subscriber holds the AppKit/UIKit view (or
`CALayer`) and sets its value: a bar's fraction, a label's text, a ring's
`strokeEnd`. SwiftUI places the view once through a representable and only
passes it the stream; `updateNSView` handles configuration (colors, the key the
stream is for), never the value. `SeekBarNSView`, `ProgressStripNSView` and
`ProgressTrackNSView` are the pattern.

**On Android:** a Compose value read during composition recomposes on every
tick; read it in a draw or layout lambda (`Modifier.drawBehind`,
`graphicsLayer`) or set a View directly.

**On Windows and Linux (Avalonia):** don't bind the value through a view model's
`INotifyPropertyChanged`. The control subscribes to the stream and sets its own
property on the UI thread, or draws it in its `Render` override and calls
`InvalidateVisual`.

**Which values are "high-frequency":** anything core sends faster than about
once a second while it is active. Counts that change when a state changes
(queued, stored, pinned) are state from tables and go through the framework like
any other row data.

**Smell:** a store or view-model property updated from a progress or position
stream and read in a SwiftUI `body`, a composable, or a binding; a
`Text("\(rate)")` fed from a throughput stream.
