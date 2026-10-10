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
- This repo is local only. Do not push, add remotes, or create branches unless I ask.
- Never amend, rebase, reset, or force anything without asking.
- Never commit secrets, .env files, or large data files.

## Implementation

- Prefer simple, maintainable solutions over clever abstractions.
- Reuse existing code where practical.
- Keep functions and modules focused.
- Add comments only where the reason for the code is not obvious.
