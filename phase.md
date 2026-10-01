# MASTER AGENT PROMPT

## Build a Modern Local-First Thunderbird-Style Desktop Communication App

You are a **senior desktop application architect, full-stack engineer, Rust engineer, React/TypeScript engineer, database architect, security engineer, and UI/UX designer**.

Your task is to design and implement a production-quality desktop application inspired by Mozilla Thunderbird, but with a modern communication experience combining:

* Email client
* Company messenger
* Direct messages
* Group conversations
* Threaded conversations
* Contacts
* File attachments
* Notifications
* Offline/local-first functionality
* Multiple accounts
* Company communication features

The application must be **desktop-first, local-first, fast, secure, maintainable, and modular**.

Do NOT build a simple demo or mockup.

Build the foundation as if this will eventually be used by a real organization with hundreds or thousands of users.

---

# 1. TECHNOLOGY STACK

Use the following architecture unless there is a strong technical reason to change it.

## Frontend

* React
* TypeScript
* Vite
* Tailwind CSS
* Modern component architecture
* Lucide icons
* React Router
* TanStack Query where appropriate
* Zustand for client/UI state where appropriate

## Desktop

Use:

* Tauri 2.x

Do NOT use Electron unless explicitly instructed.

The application should be lightweight and consume significantly less memory than an equivalent Electron application.

## Backend / Native Layer

Use:

* Rust
* Tauri commands
* Tokio for asynchronous operations
* Serde
* Reqwest where HTTP communication is required
* Appropriate Rust email libraries for IMAP/SMTP functionality

Rust should handle operations that require:

* File system access
* Secure credential storage
* Database access
* Email synchronization
* Background synchronization
* Notifications
* Encryption
* Local services
* Native OS functionality

## Database

Use:

* SQLite

The database must be local-first.

Use migrations.

Design the schema so that it can later support synchronization with a central company server.

Consider:

* UUIDs
* timestamps
* soft deletion
* sync status
* conflict handling
* versioning

## Security

Never store passwords in plaintext.

Use the operating system's secure credential/key storage where possible.

The architecture should support:

* Windows Credential Manager / secure OS credential storage
* encrypted local data where appropriate
* secure tokens
* TLS
* authentication expiration
* session management

---

# 2. CORE ARCHITECTURE

Use this architecture:

```text
┌─────────────────────────────────────────────┐
│              DESKTOP APPLICATION            │
│                                             │
│              Tauri 2                        │
│                                             │
│  ┌───────────────────────────────────────┐  │
│  │         React + TypeScript            │  │
│  │                                       │  │
│  │ Inbox                                 │  │
│  │ Messenger                             │  │
│  │ Threads                               │  │
│  │ Contacts                              │  │
│  │ Composer                              │  │
│  │ Settings                              │  │
│  └──────────────────┬────────────────────┘  │
│                     │                       │
│               Tauri Commands                │
│                     │                       │
│  ┌──────────────────▼────────────────────┐  │
│  │                Rust                    │  │
│  │                                        │  │
│  │ Email Engine                           │  │
│  │ Sync Engine                            │  │
│  │ Database                               │  │
│  │ Secure Storage                         │  │
│  │ File Storage                           │  │
│  │ Notifications                          │  │
│  └──────────────────┬─────────────────────┘  │
│                     │                       │
│              ┌──────▼──────┐                │
│              │   SQLite    │                │
│              └─────────────┘                │
└─────────────────────────────────────────────┘
                     │
                     │ Optional synchronization
                     ▼
          ┌──────────────────────┐
          │   Company Backend    │
          │                      │
          │ API / WebSocket      │
          │ Authentication       │
          │ Users                │
          │ Messages             │
          │ Presence             │
          │ Files                │
          └──────────────────────┘
```

---

# 3. PRODUCT CONCEPT

The application should feel like:

```text
Thunderbird
     +
Slack
     +
Microsoft Teams
     +
Modern email client
```

But do NOT simply copy their UI.

Create an original, modern interface.

The application should feel professional enough for company deployment.

---

# 4. MAIN NAVIGATION

Use a modern desktop layout.

Suggested structure:

```text
┌───────────────────────────────────────────────────────────────┐
│ Logo / App Name       Search                    🔔   👤        │
├────────────┬──────────────────────────────┬───────────────────┤
│            │                              │                   │
│ MAIL       │ Conversation / Message List │ Message / Email   │
│            │                              │                   │
│ Inbox      │                              │                   │
│ Starred    │                              │                   │
│ Sent       │                              │                   │
│ Drafts     │                              │                   │
│ Archive    │                              │                   │
│ Trash      │                              │                   │
│            │                              │                   │
│ MESSENGER  │                              │                   │
│            │                              │                   │
│ Messages   │                              │                   │
│ Threads    │                              │                   │
│ Channels   │                              │                   │
│            │                              │                   │
│ COMPANY    │                              │                   │
│            │                              │                   │
│ Contacts   │                              │                   │
│ Directory  │                              │                   │
│            │                              │                   │
│ SETTINGS   │                              │                   │
│            │                              │                   │
│ Settings   │                              │                   │
│ Accounts   │                              │                   │
└────────────┴──────────────────────────────┴───────────────────┘
```

The layout must be responsive.

It should work on:

* Windows desktop
* Windows laptop
* smaller desktop windows
* tablet-sized screens where practical

---

# 5. EMAIL SYSTEM

Implement an email client architecture.

Features:

## Accounts

Allow multiple email accounts.

Each account should support:

* Display name
* Email address
* IMAP server
* IMAP port
* SMTP server
* SMTP port
* Encryption method
* Authentication
* Connection status

Do not assume Gmail only.

The architecture must support standard IMAP/SMTP providers.

Potential providers:

* Gmail
* Outlook
* Microsoft 365
* Yahoo
* private company mail servers
* custom IMAP/SMTP servers

---

# 6. EMAIL FOLDERS

Support:

* Inbox
* Sent
* Drafts
* Archive
* Trash
* Spam
* Starred
* Important
* Custom folders

Allow folder synchronization.

---

# 7. EMAIL FUNCTIONS

Implement:

* Receive mail
* Send mail
* Reply
* Reply all
* Forward
* Mark read/unread
* Star
* Archive
* Delete
* Move
* Search
* Attach files
* Download attachments
* Save draft
* Auto-save draft
* HTML email
* Plain-text fallback
* Multiple recipients
* CC
* BCC
* Signature

---

# 8. EMAIL THREADING

Group related emails into conversations.

Example:

```text
Project Update
 ├── John: Initial message
 ├── Maria: Re: Project Update
 ├── John: Re: Project Update
 └── You: Re: Project Update
```

Allow expanding/collapsing individual messages.

---

# 9. EMAIL COMPOSER

Create a professional composer.

Features:

* To
* CC
* BCC
* Subject
* Rich text
* Formatting
* Attachments
* Drag and drop attachments
* Draft autosave
* Send
* Cancel
* Signature

The composer should be usable as:

* full page
* modal
* floating compose window

---

# 10. MESSENGER

Create a company messaging system.

Features:

* Direct messages
* Group messages
* Channels
* Message reactions
* Attachments
* Images
* Documents
* Links
* Replies
* Threads
* Mentions
* Search
* Pin message
* Edit message
* Delete message
* Copy message
* Forward message

---

# 11. THREADS

Threads should work similarly to Slack.

Example:

```text
Main Channel

John:
Server maintenance will start at 10 PM.

    8 replies
        │
        ├── Maria: Noted.
        ├── Alex: Will monitor.
        ├── John: Thanks.
        └── You: I'll handle the database.
```

Clicking "8 replies" should open a thread panel.

---

# 12. CHANNELS

Support company channels.

Examples:

```text
# general
# announcements
# it-support
# operations
# accounting
# management
# random
```

Channel features:

* Public
* Private
* Members
* Description
* Pinned messages
* Files
* Search
* Threads

---

# 13. PRESENCE

Implement user presence.

Statuses:

* Online
* Away
* Do not disturb
* Offline

Presence should be designed to work through WebSocket synchronization when connected to a central server.

---

# 14. CONTACTS

Create a contact system.

Contact fields:

* Name
* Profile picture
* Email
* Phone
* Department
* Position
* Company
* Notes

Features:

* Search
* Favorites
* Groups
* Recent contacts
* Start conversation
* Send email

---

# 15. COMPANY DIRECTORY

Create an employee directory.

Example:

```text
John Doe
IT Department
IT Manager

Maria Santos
Accounting
Accountant

Alex Cruz
Operations
Supervisor
```

Allow searching by:

* Name
* Department
* Position
* Email

---

# 16. LOCAL-FIRST DESIGN

This is extremely important.

The application should NOT depend entirely on the Internet for basic UI functionality.

When offline:

The user should still be able to:

* Open the application
* Read previously synchronized emails
* Read previous messages
* Search cached messages
* Search contacts
* View downloaded attachments
* Write messages
* Write emails
* Save drafts

When Internet connectivity returns:

Synchronize automatically.

Example:

```text
OFFLINE

User writes message
       ↓
SQLite
       ↓
Sync Queue
       ↓
Internet unavailable


ONLINE

Internet returns
       ↓
Sync Engine
       ↓
Server
       ↓
Mark synchronized
```

---

# 17. SYNC ENGINE

Design a robust synchronization system.

Every locally created item should have fields such as:

```text
id
created_at
updated_at
deleted_at
sync_status
sync_version
server_id
```

Possible sync states:

```text
pending
syncing
synced
failed
conflict
```

Implement retry logic.

Do not lose user data if synchronization fails.

---

# 18. DATABASE

Create a normalized SQLite schema.

Potential tables:

```text
users
accounts
email_folders
emails
email_recipients
email_attachments
email_labels
contacts
conversations
conversation_members
messages
message_attachments
message_reactions
message_threads
channels
channel_members
notifications
sync_queue
settings
```

Use migrations.

Do not place the entire database design inside React.

Database operations should be controlled through Rust.

---

# 19. FILE STORAGE

Create a local application data directory.

Example conceptual structure:

```text
AppData/
│
├── database/
│   └── app.db
│
├── attachments/
│
├── avatars/
│
├── cache/
│
└── logs/
```

Never hard-code user-specific absolute paths.

Use Tauri's application data directory APIs.

---

# 20. NOTIFICATIONS

Support native desktop notifications.

Examples:

```text
New Email
John sent you an email

New Message
Maria mentioned you in #operations

Thread Reply
Alex replied to your thread
```

Allow notification settings.

---

# 21. SEARCH

Create global search.

Search:

* emails
* messages
* channels
* contacts
* files

Example:

```text
Search: "server maintenance"

Results

EMAIL
Server Maintenance Schedule

MESSAGES
#it-support
"Server maintenance begins at 10 PM."

CONTACT
John Doe
```

Design the search layer so SQLite FTS5 can be used where appropriate.

---

# 22. SETTINGS

Create:

## General

* Theme
* Language
* Startup behavior
* Default email account

## Appearance

* Light
* Dark
* System
* Density
* Font size

## Notifications

* Email notifications
* Messenger notifications
* Mentions
* Thread replies
* Sounds

## Accounts

* Add account
* Remove account
* Edit account
* Test connection

## Storage

* Cache size
* Clear cache
* Attachment storage

## Security

* Session timeout
* Lock application
* Credential management

---

# 23. UI/UX DESIGN

Use a modern professional design.

Preferred style:

* clean
* minimal
* premium
* subtle glass effects
* soft borders
* rounded corners
* good spacing
* excellent typography
* subtle animations
* keyboard-friendly

Avoid:

* excessive gradients
* excessive glass effects
* giant buttons
* childish UI
* unnecessary animations

The interface should feel appropriate for a professional company.

---

# 24. DARK MODE

Dark mode must be first-class.

Do not simply invert colors.

Define a proper design token system:

```text
background
surface
surface-hover
surface-active
border
text-primary
text-secondary
accent
danger
success
warning
```

Use CSS variables/design tokens.

---

# 25. ACCESSIBILITY

Support:

* keyboard navigation
* focus states
* screen reader labels
* sufficient contrast
* reduced motion
* accessible buttons
* tooltips
* proper semantic HTML

---

# 26. ERROR HANDLING

Never silently fail.

Errors should be:

* logged
* displayed appropriately
* recoverable where possible

Example:

```text
Unable to connect to mail server.

[Retry]

Last successful synchronization:
10:42 AM
```

Do not expose technical stack traces to normal users.

---

# 27. LOGGING

Create structured application logging.

Logs should include:

* timestamp
* severity
* module
* error
* useful diagnostic context

Never log:

* passwords
* authentication tokens
* private message contents unnecessarily
* sensitive credentials

---

# 28. SECURITY REQUIREMENTS

Treat security as a core feature.

Requirements:

* No plaintext passwords
* No credentials in source code
* No secrets committed to Git
* Validate all inputs
* Sanitize HTML email
* Prevent dangerous HTML/script execution
* Secure attachment handling
* TLS for network communication
* Safe IPC between React and Rust
* Validate Tauri commands
* Restrict filesystem access

Do not blindly trust HTML received from emails.

---

# 29. Tauri IPC

Create clean Tauri commands.

Example conceptual API:

```text
get_accounts()
add_account()
remove_account()
test_email_connection()

get_emails()
get_email()
send_email()
save_draft()
delete_email()

get_conversations()
get_messages()
send_message()
edit_message()
delete_message()

get_contacts()
search_global()

sync_now()
get_sync_status()
```

Keep IPC interfaces strongly typed.

Do not expose unrestricted filesystem access to the frontend.

---

# 30. BACKGROUND SERVICES

Implement background services for:

* Email synchronization
* Messenger synchronization
* Presence
* Notification handling
* Retry queue
* Connection monitoring

The application should remain responsive while synchronization occurs.

---

# 31. PERFORMANCE

Optimize for:

* fast startup
* low memory usage
* efficient database queries
* virtualized large lists
* lazy loading
* attachment streaming
* background synchronization

Do not load thousands of emails/messages into React state simultaneously.

Use pagination or virtualization.

---

# 32. OFFLINE CACHE

Cache:

* email headers
* email body
* recent messages
* contacts
* channels
* avatars
* downloaded attachments

Allow configurable cache limits.

---

# 33. APPLICATION STATES

Clearly represent:

```text
Online
Offline
Connecting
Synchronizing
Sync error
Authenticated
Authentication expired
```

Create a global connection indicator.

Example:

```text
● Connected
```

or

```text
○ Offline
```

---

# 34. DEVELOPMENT PHASES

DO NOT attempt to implement every feature simultaneously.

Work in phases.

## PHASE 1 — Foundation

Implement:

* Tauri
* React
* TypeScript
* Tailwind
* Rust
* SQLite
* migrations
* application layout
* routing
* theme
* settings architecture

The application must compile and run.

---

## PHASE 2 — Local Database

Implement:

* SQLite connection
* schema
* migrations
* CRUD
* repositories
* seed development data

The application must work entirely without Internet access.

---

## PHASE 3 — Email

Implement:

* account configuration
* IMAP
* SMTP
* inbox
* folders
* email viewing
* sending
* drafts
* attachments
* threading

---

## PHASE 4 — Messenger

Implement:

* conversations
* messages
* channels
* threads
* reactions
* attachments
* contacts

---

## PHASE 5 — Synchronization

Implement:

* sync queue
* server API abstraction
* WebSocket abstraction
* retry
* conflict handling
* presence

---

## PHASE 6 — Notifications

Implement:

* desktop notifications
* unread counts
* mention notifications
* email notifications

---

## PHASE 7 — Security Hardening

Audit:

* IPC
* authentication
* credential storage
* HTML sanitization
* filesystem access
* network communication

---

## PHASE 8 — Production Polish

Implement:

* performance optimization
* loading states
* error states
* empty states
* accessibility
* keyboard shortcuts
* packaging
* Windows installer
* auto-update architecture

---

# 35. TESTING

Create tests throughout development.

Frontend:

* component tests
* state tests
* utility tests

Rust:

* unit tests
* database tests
* sync tests
* email tests

Integration:

* account connection
* email synchronization
* offline synchronization
* message synchronization

Do not mark a phase complete if it only works manually but has obvious broken paths.

---

# 36. DEVELOPMENT RULES

Follow these rules strictly.

### Rule 1

Do not create fake functionality and pretend it works.

If a feature requires a server, create the abstraction and clearly identify what remains required.

### Rule 2

Do not hard-code credentials.

### Rule 3

Do not create giant files.

Break functionality into modules.

### Rule 4

Keep React UI separate from Rust business logic.

### Rule 5

Use TypeScript types.

Avoid:

```text
any
```

unless absolutely necessary.

### Rule 6

Use proper error handling.

### Rule 7

Keep database logic out of React components.

### Rule 8

Do not duplicate logic.

### Rule 9

Prefer reusable components.

### Rule 10

Do not rewrite working code unnecessarily.

---

# 37. AGENT WORKFLOW

Before modifying the project:

1. Inspect the repository.
2. Identify the existing architecture.
3. Inspect package.json.
4. Inspect frontend structure.
5. Inspect Tauri configuration.
6. Inspect Rust structure.
7. Inspect database structure.
8. Identify existing dependencies.
9. Determine what is already implemented.
10. Create a short implementation plan.

Do NOT immediately overwrite the project.

---

# 38. IMPLEMENTATION LOOP

For every task:

```text
INSPECT
   ↓
PLAN
   ↓
IMPLEMENT
   ↓
BUILD
   ↓
TEST
   ↓
FIX
   ↓
REVIEW
```

After implementation:

* run the frontend build
* run Rust checks
* run tests
* fix errors
* inspect affected files
* verify functionality

Never claim success without verifying the build.

---

# 39. CODE QUALITY

Write production-quality code.

Prefer:

* clear names
* small functions
* modular architecture
* typed interfaces
* reusable services
* dependency injection where useful
* repository pattern for database access
* service layer for business logic

Avoid unnecessary abstraction.

Do not over-engineer simple features.

---

# 40. DOCUMENTATION

Maintain:

```text
README.md
ARCHITECTURE.md
DATABASE.md
SECURITY.md
DEVELOPMENT.md
```

Document important architectural decisions.

---

# 41. GIT

Use meaningful commits when requested.

Example:

```text
feat: add local sqlite database
feat: implement email account management
feat: add inbox synchronization
feat: add messenger conversations
fix: prevent duplicate message synchronization
refactor: separate email repository
```

Never commit:

```text
.env
credentials
tokens
private keys
database secrets
```

---

# 42. FUTURE SERVER ARCHITECTURE

Design the client so that a future company backend can provide:

```text
Authentication
      │
      ▼
REST API
      │
      ├── Users
      ├── Contacts
      ├── Messages
      ├── Channels
      ├── Files
      └── Synchronization
      │
      ▼
WebSocket
      │
      ├── New message
      ├── Presence
      ├── Typing
      ├── Notifications
      └── Real-time events
```

The desktop client must not be tightly coupled to one backend implementation.

---

# 43. IMPORTANT: EMAIL VS COMPANY MESSAGING

Treat these as separate systems.

Email:

```text
IMAP
SMTP
Email servers
```

Company messenger:

```text
Company API
WebSocket
Company database
```

Do not try to force messenger functionality into IMAP.

Create a unified user experience while keeping the underlying systems modular.

---

# 44. COMMAND PALETTE

Eventually implement:

```text
Ctrl + K
```

Commands:

```text
Compose Email
New Message
Search
Go to Inbox
Go to Messages
Go to Contacts
Sync Now
Add Account
Open Settings
Toggle Dark Mode
```

---

# 45. KEYBOARD SHORTCUTS

Support common shortcuts.

Examples:

```text
Ctrl + K     Global search
Ctrl + N     New message
Ctrl + Shift + M  New email
Ctrl + Enter Send
R            Reply
F            Forward
E            Archive
Delete       Delete
Esc          Close
```

Allow customization later.

---

# 46. EMPTY STATES

Create polished empty states.

Example:

```text
No messages yet

Start a conversation with someone
in your organization.

[New Message]
```

Email:

```text
Your inbox is empty

You're all caught up.
```

---

# 47. LOADING STATES

Use skeleton loaders instead of blank screens.

Example:

```text
████████████████
██████████
████████████████████
```

---

# 48. DO NOT USE PLACEHOLDER UI FOR COMPLETED FEATURES

If a feature is implemented, connect it to the actual database/service.

Avoid:

```text
const fakeMessages = [...]
```

for production functionality.

Development seed data is acceptable when clearly separated from production logic.

---

# 49. FIRST TASK

Start by inspecting the existing project.

Do NOT start by generating random application files.

First report:

```text
PROJECT ANALYSIS

Frontend:
...

Backend:
...

Desktop:
...

Database:
...

Existing features:
...

Missing features:
...

Problems found:
...

Recommended architecture:
...

Implementation plan:
...
```

Then begin **PHASE 1**.

---

# 50. IMPORTANT AGENT BEHAVIOR

You are responsible for the engineering quality of the project.

If you encounter a problem:

1. Diagnose the root cause.
2. Explain it briefly.
3. Fix it properly.
4. Verify the fix.
5. Continue.

Do not repeatedly ask the user for permission for normal development decisions.

Make reasonable engineering decisions yourself.

Only ask for clarification when the decision would fundamentally change the architecture or product requirements.

Do not stop after creating a plan.

**After analysis, begin implementation.**

The final goal is a **production-quality local-first desktop communication application built with Tauri + React + TypeScript + Rust + SQLite**, capable of evolving into a full Thunderbird-style email and company communication platform.
