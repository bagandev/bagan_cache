# Bagan Cache

Run the filesystem watcher from the project root with:

```sh
cargo run --example cache -- [entry-directory]
```

The entry directory defaults to the current directory and must contain a
supported `index` Markdown file. The default theme must be installed under
`bagan_themes` in the project root. Press Enter to stop watching and flush
pending cache writes.
