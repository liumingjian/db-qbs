---
status: proposed
---

# Use two fixed shared accounts for role-based access

Email delivery settings introduce system-wide configuration that ordinary operators must not
change. The source service will therefore have exactly two shared accounts with immutable roles:
`admin` is the Administrator and `operator` is the Operator. This adds authorization without
introducing user registration, account CRUD, role assignment, task ownership, per-user visibility,
or individual audit identity.

Administrators may perform every operation and exclusively manage datasources, Target Agents,
email alert settings, and the Operator account. Operators may view datasource and Target Agent
state, manage and run shared tasks, inspect run history and logs, and change their own password.
Authorization is enforced by the backend; hiding controls in the web interface is not a security
boundary.

The existing `admin` account remains the Administrator during upgrade. The `operator` account is
disabled until an Administrator sets its password in the web interface. Either account may change
its own password after supplying the current password. An Administrator may enable, disable, or
reset the Operator account; disabling or resetting it invalidates all Operator sessions. A password
change invalidates the account's other sessions. Administrator recovery remains a host-side CLI
operation.

## Considered Options

A full named-user and role-management model was rejected because the deployment uses shared
operational identities and needs only a boundary around system configuration. Keeping the existing
single Administrator account was rejected because ordinary task operation would still require
sharing system-configuration authority.

## Consequences

Tasks remain shared and have no owner. The system can attribute an action to the Administrator or
Operator account, but cannot identify the individual person who performed it; no individual audit
trail is promised.
