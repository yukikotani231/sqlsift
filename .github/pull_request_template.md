<!--
Thanks for contributing! Please read CONTRIBUTING.md first.
PR titles use Conventional Commits (feat:, fix:, docs:, ...).
-->

## Related issue

<!-- e.g. "Closes #123". For larger changes, please discuss the approach in an issue first. -->

## Summary

<!-- What changes for a user of sqlsift? A short before / after works well, e.g. the SQL, and what sqlsift reported before and reports now. -->

## How it was tested

<!-- New or changed tests, and anything you checked by hand. -->

## Checklist

- [ ] I ran these locally and they pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
- [ ] Tests cover the change (a query that must be reported and one that must stay clean, where it applies)
- [ ] The user guide (`docs/`), README or sample `sqlsift.toml` is updated if behavior or options changed
- [ ] If an AI assistant wrote part of this PR, I have read and understood every change and can answer review questions about it
