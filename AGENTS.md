# Project Instructions

## project info
Datadiver is a data exploration tool and quick lookup tool.
Goals:
  - I can trust the data output, nothing is missing or duplicated
  - Be enjoyable to use by being very efficient and fast
  - I want to use keyboard shortcuts that are similar to VS Code when possible. 
  - To make the program easy to use and easy to find things, I want to integrate some help information in there. For example, for the search we have input, we have a mini language, and I want to find a way to explain things like that in the program.  I think that having the command palette being nice and searchable like it is is one thing we are already doing that makes things easy to find. 

## presistence

This app is still in the development stage and has not been deployed anywhere. So when we make updates that impact the persistence, we do not need to worry about how the prior persistence settings will be impacted. We can manually delete them. 


## Codebase navigation

This project uses open-codebase-index.

Before broadly reading or grepping source files, use the codebase index to locate the relevant code.

Preferred workflow:
1. Use `codebase_context` to understand the relevant area of the repository.
2. Use `codebase_peek` to identify likely files/symbols when only locations are needed.
3. Use `implementation_lookup` for known structs, functions, traits, or methods.
4. Use `codebase_search` when actual matching source context is needed.
5. Read only the specific source files/ranges identified as relevant.
6. Use grep/ripgrep for exact identifiers or exhaustive text searches.

Avoid reading large files or many files speculatively when the index can narrow the search first.


## Workflow

- Follow the existing architecture and coding conventions
- Do not add dependencies unless they are necessary.
- Do not modify unrelated files.


## Verification

- After each logical phase, run the appropriate tests.
- Run the full test suite before considering the task complete.
- Fix warnings or errors introduced by your changes.
- run release builds at project completion or before a release, not after every small change.
- Do not claim something works unless it has been tested when testing is
  reasonably possible.

## Seeing the UI

The binary can open any UI surface and screenshot itself, so a change to the
interface can be checked without a human at the keyboard. Build the capture
feature and point `--capture` at a PNG:

```powershell
cargo run --features snapshots -- --rows 60 --show info `
    --capture ui/info.png --ephemeral
```


## Git workflow

- After completing each change or task, commit it automatically. Don't ask first.
- Stage only the files related to that change. Never use `git add -A` or `git add .` if there are unrelated modifications.
- Write commit messages in imperative mood ("Add parser for X", not "Added..."), with a short summary line under 72 characters and a body explaining why if the change isn't obvious.
- Make small, focused commits: one logical change per commit.
- Work on `main`. The public repo is the `github` remote (github.com/ryanwd123/datadiver). `origin` is an old local copy and `master` is a private archive of the full history; never push either.
- Push to `github` only when I ask. Do not add remotes or create branches unless I ask.
- Never amend, rebase, reset, or force anything without asking.
- Never commit secrets, .env files, or large data files.

## Releasing

Releases are tag-driven. Pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml`, which builds the wheel with maturin using the `dist` profile, creates the GitHub release with the wheel and a zipped exe, and publishes to PyPI through trusted publishing. Only release when I ask.

1. Make sure `main` is clean, pushed, and the latest CI run passed (`gh run list --limit 1`).
2. Run the full test suite, both `cargo test` and `cargo test --features snapshots`.
3. Run `.\scripts\release.ps1 X.Y.Z`. It bumps the version in `Cargo.toml`, runs the tests, commits "Release vX.Y.Z", tags, and pushes. Use semver: patch for fixes, minor for new features. Ask me if the version isn't clear.
4. Follow the build with `gh run watch` (about 30 minutes, mostly compiling DuckDB).
5. Check that the release has both assets (`gh release view vX.Y.Z`) and that the wheel is on PyPI. Use `https://pypi.org/simple/datadiver/`, since the JSON API can return a cached 404 for a while.

Rules:

- `Cargo.toml` is the only place the version lives. The exe's version info and the wheel version both come from it.
- Don't tag by hand, and never delete, move, or re-push a tag that has been pushed without asking me.
- PyPI never accepts the same version twice. If a release fails after its tag is pushed, fix the problem and release the next patch version.
- `gh` is installed at `C:\Program Files\GitHub CLI\gh.exe` and may not be on the shell's PATH.

## Implementation

- Prefer simple, maintainable solutions over clever abstractions.
- Reuse existing code where practical.
- Keep functions and modules focused.
- Add comments only where the reason for the code is not obvious.
