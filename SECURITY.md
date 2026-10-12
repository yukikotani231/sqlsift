# Security Policy

## Supported versions

Security fixes go into the latest release. Please upgrade before reporting.

## Reporting a vulnerability

Please do not open a public issue for security problems. Report them privately through
[GitHub's vulnerability reporting](https://github.com/yukikotani231/sqlsift/security/advisories/new)
instead.

Include what is affected (CLI, LSP server, VS Code extension, GitHub Action or playground),
the version, and steps to reproduce. You should get a reply within a week. Once a fix is
released, the advisory is published and you are credited unless you prefer not to be.

## Scope

sqlsift only reads SQL and schema files and never connects to a database. Reports that
matter most are about running untrusted input: crashes or hangs on crafted SQL, files
read or written outside the project, and problems in the release pipeline, the npm
package, the VS Code extension or the GitHub Action.
