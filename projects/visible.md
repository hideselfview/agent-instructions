# visible

visible is a home inventory app. It organizes the physical things in a house as
a tree that the person browses: the house at the root, then rooms, containers
and single things, each with photos. It also keeps the household's task list. A
Rust core (`visible-core`) does the work, and native apps show it through a
uniffi bridge (`visible-bridge`). The library is stored and synced through
coven, the end-to-end-encrypted SQLite sync layer bae also uses, and the person
shares it with the people they live with through an invite code.

## Crates and apps

- `visible-core` owns the domain: the node tree, image files and tasks.
- `visible-bridge` translates core types to Swift and Kotlin and nothing else.
- `visible-android` is the Android app, in Jetpack Compose.
- `visible-ios` is the iOS app, in SwiftUI.
- `visible-macos` is the macOS app.
  - It compiles the iOS app's SwiftUI sources, which branch on `#if os(...)`
    where the platforms differ (`visible-macos/visible/project.yml` lists them).

## Data

Everything the person owns is a node in one tree (the `nodes` table). The house
is the root (`parent_id` NULL), and a node's children are its contents, in
`position` order. A node's photos are `node_images` rows and its tags
`node_tags` rows, and `tasks` holds the household's task list. The schema is in
`visible-core/src/node.rs` and `visible-core/src/task.rs`.

coven owns the SQLite connection (`coven::Database::open` returns the database
and the `_updated_at` stamper every write uses), the on-disk layout
(`coven::LibraryDir`), and each library's config and device id
(`coven::config`).

## Shapes change in one commit, with no compatibility code

visible follows
`rules/necessary-code/change-a-shape-in-one-commit-with-no-compatibility-code.md`.
To reset a device's data, delete the app's data folder: Application Support on
iOS and macOS (inside the app's sandbox container on macOS), and the app's files
folder on Android.

## iOS and Android are at parity

Every user-facing capability ships on both `visible-ios` and `visible-android`,
backed by the same `visible-bridge` calls. A feature added to one platform
without the other is incomplete.

## Building the bridge

Each script cross-compiles `visible-bridge` and generates the bindings, which
are gitignored:

- `./visible-bridge/build-android.sh` builds for the Android NDK targets,
  generates the Kotlin bindings, and copies the `.so` files into
  `visible-android/app/src/main/jniLibs/`.
- `./visible-bridge/build-ios.sh` builds for device and simulator, generates the
  Swift bindings, and packages `visible-ios/VisibleBridgeFFI-ios.xcframework`.
- `./visible-bridge/build-macos.sh` builds for this Mac and copies the Swift
  bindings into `visible-macos/visible/visible/`.
  - `./visible-macos/run.sh` runs it, then builds the macOS app and opens it
    (`--no-open` builds only).
