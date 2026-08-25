app-title = Envelope
about = About
settings = Settings
accounts = Accounts

# Sidebar
no-folders = No folders yet. Set up a mail server, then sync.

# Account setup
no-accounts = No accounts configured.
no-accounts-description = Envelope shares the suite's accounts. Add one in Slate or Circle and it appears here, with its password — then give it a mail server below.
no-mail-server = No mail server set
set-up-mail = Set up
change = Change
mail-server = Mail server
imap-host = IMAP server
port = Port
encryption = Encryption
username = Username
imap-username-hint = Only if it differs from the account's.
save = Save
cancel = Cancel
sync-now = Sync now
syncing = Syncing…
bad-port = That is not a port number.
no-mail-account = This account has no mail server yet.
sync-summary = { $fetched } new, { $pushed } sent.
sync-failed = Sync failed: { $reason }
sync-partial = { $count ->
        [one] One folder could not be synced.
       *[other] { $count } folders could not be synced.
    }
sync-stuck = { $count ->
        [one] One change cannot reach the server. Check the account's password.
       *[other] { $count } changes cannot reach the server. Check the account's password.
    }

# Message list
loading = Loading…
empty-folder = Nothing in this folder.
no-message-selected = Select a conversation.
no-subject = (no subject)
unknown-sender = Unknown sender
to-line = To { $recipients }

# Reader
mark-read = Mark read
mark-unread = Mark unread
star = Star
unstar = Unstar
archive = Archive
delete = Delete
attachments = Attachments
auth-fail = This message was not sent by { $domain }, whatever it says. Treat it as forged.
auth-partial = This message could not be fully verified as coming from its sender.
hidden-content = { $count ->
        [one] This message hides one thing from you that it still says in its source.
       *[other] This message hides { $count } things from you that it still says in its source.
    }
remote-content-blocked = This message wanted to load images from a server, which would have told the sender you opened it. Envelope did not.
no-archive-folder = This server has no archive folder.

# Sending
sending-section = Sending
smtp-host = Outgoing server
smtp-host-hint = Only if it differs from the incoming one.
smtp-port = Outgoing port
smtp-encryption = Outgoing encryption
from-address = Send from
from-address-hint = The address recipients see. Leave empty to use the login.
from-name = Your name
no-from-address = This account has no address to send from.
compose = New message
reply = Reply
reply-all = Reply all
forward = Forward
to = To
cc = Cc
bcc = Bcc
subject = Subject
send = Send
sending = Sending…
discard = Discard
sent = Sent.
sent-not-filed = Sent, but the copy could not be filed in Sent.
send-failed = Not sent: { $reason } Nothing was delivered, so you can try again.
send-uncertain = This may or may not have been delivered: { $reason } Check your Sent folder or ask the recipient before sending it again.
draft-not-saved = The draft could not be saved: { $reason }
draft-gone = That draft is no longer there.
drafts = Drafts
no-drafts = No saved drafts.
draft-no-subject = (no subject)
save-draft = Save
drafts-are-local = Kept on this device.
search = Search
no-results = Nothing matched.
searching = Searching…
results-capped = Showing the first { $count }. Narrow the search to see fewer.
search-hint = Searches senders, subjects, and message bodies, best match first.
hit-folder-gone = That message's folder is no longer on the server.
in-folder = in { $folder }
save = Save
saving = Saving…
attach = Attach
remove = Remove
attachment-saved = Saved to { $path }
attachment-not-saved = Could not save it: { $reason }
attachment-size = { $size }
attachments-total = { $count ->
        [one] One file, { $size }
       *[other] { $count } files, { $size }
    }
find-settings = Find settings
finding-settings = Looking…
found-known = These are the published settings for this provider.
found-autoconfig = Found where this domain publishes its settings.
found-guessed = Guessed from the domain — something answered, but check them.
send-queued = In the outbox. It will go out on the next check.
outbox = Outbox
outbox-empty = Nothing waiting to go.
outbox-stopped = Stopped after too many tries.
try-again = Try again
sync-sent = { $count ->
        [one] One sent.
       *[other] { $count } sent.
    }

# Actions and shortcuts
next-message = Next
previous-message = Previous
toggle-read = Mark read or unread
toggle-starred = Star or unstar
close = Close
go-inbox = Inbox
go-drafts = Drafts
go-outbox = Outbox
go-sent = Sent
go-archive = Archive
shortcuts = Keyboard shortcuts
group-reading = Reading
group-writing = Writing
group-going = Going to
group-application = The application
shortcuts-hint = Single keys work whenever you are not typing. The combinations always work.
no-such-folder = This server has no folder of that kind.
repository = Repository
choose-files = Choose files to attach
no-file-dialog = The file chooser could not be opened: { $reason }
check-every = Check for mail every
check-every-hint = In seconds. Anything below { $minimum } is treated as { $minimum } — a client that checks faster than that looks broken to a server.
mark-read-on-open = Mark read when opened
mark-read-on-open-hint = Turn this off if you use your inbox as a to-do list.
protocol = Protocol
protocol-hint = IMAP unless your provider says otherwise. POP3 downloads and keeps no folders.
jmap-url = JMAP session URL
jmap-url-hint = Only for JMAP. Your provider publishes it; Fastmail's is filled in for you.
found-provider = These are this provider's published settings, protocol included.
sign-in = Sign in with a provider
sign-in-address = Your address there
sign-in-with = Sign in with { $provider }
sign-in-waiting = Waiting for the browser…
sign-in-browser = Finish signing in, in your browser. This page will update by itself.
sign-in-needs-address = Type the address you are signing in with first.
sign-in-failed = The sign-in did not finish: { $reason }
command-palette = Command palette
palette-placeholder = Type a command…
palette-nothing = Nothing matches.

# Undo
undo = Undo
undo-flags = the flag change
undo-move = the move to { $folder }
undone = Took back { $what }.
undo-failed = Could not take that back: { $reason }
nothing-to-undo = Nothing to take back.

# Lists and export
unsubscribe = Unsubscribe
unsubscribing = Asking the sender to take you off the list…
unsubscribed = Off the list. The sender has up to a few days to comply.
unsubscribe-failed = The unsubscribe did not go through: { $reason }
save-as-file = Save as file
all-inboxes = All inboxes

# Import
import-mbox = Import an mbox archive…
choose-mbox = Choose an mbox file
importing = Uploading the archive to this folder…
imported = { $imported ->
        [one] One message imported. Syncing it down.
       *[other] { $imported } messages imported. Syncing them down.
    }
imported-some = { $imported } imported, { $skipped ->
        [one] one unreadable chunk skipped.
       *[other] { $skipped } unreadable chunks skipped.
    } Syncing them down.
