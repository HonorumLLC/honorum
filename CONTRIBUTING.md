# Contributing to Honorum

Honorum is not accepting outside contributions right now. Pull requests from
outside the maintainer team are closed without review. The source is public
so you can read it, run it, and adapt it under the [license](LICENSE).

You can still help:

- Report a bug in an issue. Search existing issues first. Describe the problem
  and the expected result. Use synthetic data in examples and screenshots.
- Report a security defect privately, as the [security policy](SECURITY.md)
  describes. Never report one in a public issue.

## Set up the repository

Install Rust 1.90 or later, Node.js 24, Make, and Docker or Podman.
Native iOS work also needs macOS and Xcode.

```sh
git clone https://github.com/HonorumLLC/honorum.git
cd honorum
make setup
npm --prefix apps/web exec -- playwright install chromium
make demo
```

Open `http://localhost:18080`. The demo uses recorded school data and a local database.
It needs no account or provider credentials.
See [development](docs/development.md) for hot reload and focused checks.

Maintainers follow [Git workflow](docs/git.md) and [AGENTS.md](AGENTS.md).

The repository uses the [GNU AGPL version 3](LICENSE).
Its files use that license unless a file states another.
