# Summary

[Introduction](introduction.md)

# Getting started

- [Installation](installation.md)
- [Quick start](quick-start.md)

# User guide

- [Loading your schema](guide/schema.md)
- [Checking queries](guide/queries.md)
- [Rules and levels](guide/rules.md)
- [Suppressing diagnostics](guide/suppression.md)
- [Dialects and SQL support](guide/sql-support.md)
- [Troubleshooting](guide/troubleshooting.md)

# Integrations

- [Example projects](integrations/examples.md)
- [CI and GitHub Actions](integrations/ci.md)
- [Pre-commit hooks](integrations/pre-commit.md)
- [Editors](integrations/editors.md)

# Reference

- [Command line](reference/cli.md)
- [Configuration file](reference/config.md)
- [Output formats](reference/output-formats.md)
- [Rules](rules/index.md)
  - [E0001 table-not-found](rules/E0001.md)
  - [E0002 column-not-found](rules/E0002.md)
  - [E0003 type-mismatch](rules/E0003.md)
  - [E0004 potential-null-violation](rules/E0004.md)
  - [E0005 column-count-mismatch](rules/E0005.md)
  - [E0006 ambiguous-column](rules/E0006.md)
  - [E0007 join-type-mismatch](rules/E0007.md)
  - [E0008 missing-required-column](rules/E0008.md)
  - [E0009 duplicate-name](rules/E0009.md)
  - [E0010 duplicate-target-column](rules/E0010.md)
  - [E0011 position-out-of-range](rules/E0011.md)
  - [E0012 misplaced-aggregate](rules/E0012.md)
  - [E0013 generated-column-write](rules/E0013.md)
  - [E0014 unmatched-conflict-target](rules/E0014.md)
  - [E0015 distinct-order-by](rules/E0015.md)
  - [E0016 grouping-error](rules/E0016.md)
  - [E0017 value-out-of-range](rules/E0017.md)
  - [E0018 null-comparison](rules/E0018.md)
  - [E0019 not-in-with-nulls](rules/E0019.md)
  - [E0020 outer-column-in-subquery](rules/E0020.md)
  - [E0021 missing-join-condition](rules/E0021.md)
  - [E0022 constant-condition](rules/E0022.md)
  - [E0023 outer-join-filtered](rules/E0023.md)
  - [E0024 unfiltered-write](rules/E0024.md)
  - [E0025 insert-without-columns](rules/E0025.md)
  - [E0026 select-star](rules/E0026.md)
  - [E0027 wrong-argument-type](rules/E0027.md)
  - [E0028 limit-without-order-by](rules/E0028.md)
  - [E0029 implicit-conversion](rules/E0029.md)
  - [E0030 foreign-key-join-mismatch](rules/E0030.md)
  - [E0031 join-fan-out](rules/E0031.md)
  - [E1000 parse-error](rules/E1000.md)

# How it works

- [Overview](how-it-works/index.md)
  - [Name resolution and types](how-it-works/names-and-types.md)
  - [Building the schema](how-it-works/schema.md)
  - [Preprocessing and locations](how-it-works/preprocessing.md)
  - [Testing and the real-world corpus](how-it-works/testing.md)

---

[Contributing](contributing.md)
