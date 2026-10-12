# Agent projects and session registration

Status: proposed. Automatic chosen-folder indexing already exists; session
registration and MCP document writes below are not implemented or advertised by
the current skill.

## Product experience

MDReader is the place to read, find and organize Markdown agents create. Connect
Codex or Claude Code, choose projects using a real folder picker, and watch their
Markdown appear automatically. Keep ordinary files in their original project
folders; index all selected projects in the background, including subfolders.
The index stays useful without agent reporting or an active MCP session.

Show Projects → Sessions → Documents. Session metadata is optional enrichment:
a file remains discoverable even if an agent never registers it. Agent name and
session attribution are reported metadata, not verified creator identity.
A document can belong to several sessions and appears once in the project index.

## Connection and automatic discovery

One Connect action configures the installed agent and local MDReader service.
The public skill command is portable, has no developer paths, and teaches the
agent how to use the service. Installation, service enablement and a successful
agent handshake are separate states. Display Connected only after a handshake;
retain last-seen information for inactive sessions.

Choose folders rather than installation scope on the main screen. Scope stays
under Advanced and controls client configuration location, not filesystem
access. Never scan home, disks, hidden agent histories or arbitrary reported
folders without an explicit user selection. Capture of final response text is a
separate optional advanced feature, not required to discover Markdown files.

## Session registration

Add register_session(project_id, client_session_id, agent, title) and
register_document(session_id, relative_path) through MCP. The native owner
validates the enabled project, current folder identity and existing file before
recording metadata in the durable library. Idempotent events survive restarts.
Registration never creates a new folder grant or bypasses pauses/removal.
Only project/session metadata and document associations are retained; prompts,
chat transcripts, reasoning, credentials and raw tool logs are not imported.

Teach the skill to register at the start of a task and report created/updated
Markdown. Use client hooks only where supported to improve reporting; folder
watching remains the fallback. Session titles and agent names are untrusted
labels. Access authority comes from the configured installation and project
grants, not caller-supplied session IDs or names.

## Optional reverse direction

Agents can search and read explicitly shared MDReader sources. Add separately
opted-in create/update tools for editable sources using the existing owner save
path, expected revisions, durable draft recovery and provider permissions.
Return conflicts rather than overwriting an unrelated change. Do not imply that
a read grant permits editing or that skill installation grants all-library access.
Deletion stays outside the first two-way batch.

## Focused acceptance

Use disposable projects and public Markdown. Connect both agent clients; register
a session; create a note; observe it automatically in MDReader with correct
session association; edit/save in MDReader; read the updated revision through the
agent. Also prove unreported files are indexed, restart retains metadata, paused
and removed projects reject registration, and conflicting updates preserve both
versions. Run one combined review and acceptance for this future feature batch.
