# Reverse Tag Lookup

Find LeetCode problems by hidden position-level tags such as `Staff`, `Senior Staff`, and `Principal`, plus normal topic tags such as `Monotonic Stack`.

![Reverse Tag Lookup app screenshot](docs/app-screenshot.png)

## Requirements

- Rust
- Cargo

## Run From GitHub

Clone the repo:

```sh
git clone https://github.com/breadream/reverse_tag_lookup.git
cd reverse_tag_lookup
```

Start the app:

```sh
PORT=3001 cargo run
```

Open the app:

```text
http://127.0.0.1:3001
```

Stop the app:

```sh
Ctrl+C
```

## What To Expect

The first search can take a little while because the backend builds a local cache from LeetCode. After that, searches are much faster.

The local cache is stored on disk, so later runs can reuse it between restarts.
