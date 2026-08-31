---
status: accepted
---

# Run optional preSQL atomically before append imports

An Import Task using the `APPEND` write mode may define one optional `preSQL` statement in its
`pre_sql` field. The option is available for both table-selected and custom-SQL sources. A missing,
empty, or whitespace-only value means that no preSQL runs. `CLEAR_THEN_IMPORT` does not accept the
option because that mode already deletes the whole target table before importing.

preSQL is one complete MySQL `DELETE` statement, not a SQL fragment or script. It must contain a
`WHERE` clause and delete only from the task's target table, named either without a database or with
the task's target database. The first version supports single-table `DELETE FROM ... WHERE ...`
only: functions and subqueries in the condition are allowed, while multi-table, join, and CTE deletes
are not. Comments and one trailing semicolon are allowed. The exact text is stored without
formatting or template substitution, so the statement remains directly executable in a MySQL
terminal; dynamic values must use MySQL expressions such as `CURRENT_DATE`.

The source validates the statement when the task is saved. The sink validates it again for every
run against the actual write mode, target database, and target table. Validation is structural, not
a string-prefix or regular-expression check. Switching a task from `APPEND` to
`CLEAR_THEN_IMPORT` requires confirmation and removes its preSQL rather than retaining a hidden
destructive instruction.

The sink waits until all source rows are staged and verified, then executes preSQL immediately
before the existing insert or upsert in the same target transaction. preSQL, import, and commit
succeed together; an error in either statement rolls the whole transaction back and leaves the
target table unchanged. preSQL does not alter whether the import uses insert or upsert, which
continues to follow from the target table's unique constraint.

An enabled task is presented as `APPEND + preSQL cleanup`, not as an ordinary append-only task. A
successful run records the machine-readable target effect `CLEANED_AND_SWAPPED`, the number of rows
deleted by preSQL, and a snapshot of the exact statement that ran. Later task edits cannot rewrite
that history. The system does not attempt to prove that the Oracle source query and the MySQL
deletion predicate describe the same data range; the task author owns that correspondence, and the
interface states this explicitly.

## Considered Options

Adding another write mode was rejected because preSQL is an optional preparation step for append
imports rather than a different import statement. Allowing it under `CLEAR_THEN_IMPORT` was rejected
because the subsequent whole-table delete would make it redundant. Accepting arbitrary SQL or
deletes against other tables was rejected because the task's target-table hold would no longer bound
the destructive and concurrency effects. Accepting a free-form `WHERE` fragment was rejected
because preSQL must remain a complete statement that operators can run unchanged in a MySQL
terminal.

## Consequences

The promise that `APPEND` never deletes applies only to tasks without preSQL. Every task and run
surface makes cleanup visible. Atomicity prevents a cleared-but-not-imported target, but it does not
make a mismatched deletion predicate correct. The DELETE may also hold locks and generate undo
records for the duration of the final import transaction.
