# Rules and levels

Every diagnostic comes from a rule. Each rule has a code (`E0002`), a name (`column-not-found`) and a category. You can refer to a rule by either its code or its name anywhere sqlsift takes a rule.

`sqlsift rules` lists them:

```console
$ sqlsift rules
CODE   NAME                     CATEGORY     DEFAULT  DESCRIPTION
E0001  table-not-found          correctness  error    Referenced table does not exist in schema
E0002  column-not-found         correctness  error    Referenced column does not exist in table
E0003  type-mismatch            correctness  error    Type incompatibility in expression
E0004  potential-null-violation correctness  error    Potential NOT NULL violation
E0005  column-count-mismatch    correctness  error    INSERT column count doesn't match values
E0006  ambiguous-column         correctness  error    Column reference is ambiguous across tables
E0007  join-type-mismatch       correctness  error    JOIN condition compares incompatible types
E0008  missing-required-column  correctness  error    INSERT omits a NOT NULL column without a default
E1000  parse-error              correctness  error    SQL could not be parsed
```

Each rule has its own page with examples under [Rules](../rules/index.md).

## Levels

A rule is `off`, `warn` or `error`:

- `error`: reported, and makes `sqlsift check` exit with `1`
- `warn`: reported, but doesn't fail the check (unless you set [`--max-warnings`](#ratcheting-warnings))
- `off`: not reported

## Categories

Like [oxlint](https://oxc.rs/docs/guide/usage/linter.html), every rule belongs to a category that sets its default level:

| Category | Default | Meaning |
|----------|---------|---------|
| `correctness` | error | The query fails or does something unintended |
| `suspicious` | warn | The query is most likely wrong |
| `pedantic` | off | Stricter checks that may have false positives |
| `style` | off | Conventions and readability |
| `restriction` | off | Bans on features some codebases don't want |

All current rules are in `correctness`.

## Changing levels

In `sqlsift.toml`:

```toml
[rules]
E0008 = "warn"             # by code...
ambiguous-column = "off"   # ...or by name

[categories]
suspicious = "error"
```

On the command line, `-A` (allow, i.e. off), `-W` (warn) and `-D` (deny, i.e. error) take a rule code, a rule name or a category, and can be repeated:

```bash
sqlsift check -W ambiguous-column -A E0008 queries/*.sql
```

A rule's own level wins over its category's, and command-line flags win over `sqlsift.toml`. `disable = ["E0006"]` in the file is shorthand for `E0006 = "off"` under `[rules]`.

## Ratcheting warnings

Rolling out a rule on an existing codebase? Make it a warning and cap the number of warnings, so the backlog can only shrink:

```bash
sqlsift check -W missing-required-column --max-warnings 12
```

`--max-warnings <N>` (or `max_warnings = N` in `sqlsift.toml`) makes the check fail when **more than** `N` warnings are reported. `0` fails on any warning without turning warnings into errors.
