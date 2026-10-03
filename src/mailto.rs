// SPDX-License-Identifier: GPL-3.0-only

//! `mailto:` URLs (RFC 6068).
//!
//! This is the contract that makes Envelope the desktop's mail client rather
//! than an application that happens to show mail: Circle's "send a message",
//! Slate's attendee addresses, and every `mailto:` link in every browser arrive
//! here. `resources/com.magnetaros.Envelope.desktop` registers the scheme.
//!
//! Deliberately permissive about what it accepts and strict about what it does
//! with it — see [`prefill`].

use cosmic_pim_mail::{Draft, Mailbox};

/// Turns a `mailto:` URL into a draft.
///
/// Returns `None` for anything that is not a `mailto:` URL at all; a URL that
/// is one but is malformed yields whatever could be read, because a link with a
/// broken `subject` should still open a composer addressed to the right person.
///
/// # What is deliberately not honoured
///
/// RFC 6068 permits arbitrary headers in the query string, and the security
/// consideration it raises is real: a link can set `bcc`, or `from`, or
/// anything else. A page that can make the user's mail client silently
/// blind-copy a third party on a message the user then writes and sends is a
/// genuine attack, not a hypothetical, and "the fields are visible in the
/// composer" is not a defence — nobody reads a Bcc field they did not fill in.
///
/// So only `to`, `cc`, `subject`, and `body` are read. `bcc` and every header
/// beyond those are dropped.
#[must_use]
pub fn prefill(url: &str, from: Mailbox) -> Option<Draft> {
    // Case-insensitively, as RFC 3986 reads a scheme and as `Launch` already
    // recognises one: `Mailto:` is a mailto link.
    const SCHEME: &str = "mailto:";
    let rest = url
        .get(..SCHEME.len())
        .filter(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
        .map(|_| &url[SCHEME.len()..])?;

    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));

    let mut draft = Draft::new(from);
    draft.to = addresses(&decode(path));

    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value);
        match key.to_ascii_lowercase().as_str() {
            "to" => draft.to.extend(addresses(&value)),
            "cc" => draft.cc.extend(addresses(&value)),
            "subject" => draft.subject = value,
            "body" => draft.body = value,
            // Everything else, including `bcc`, is dropped. See above.
            other => tracing::debug!(field = other, "ignoring a mailto: field"),
        }
    }

    Some(draft)
}

fn addresses(list: &str) -> Vec<Mailbox> {
    list.split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(|address| Mailbox {
            name: None,
            address: address.to_ascii_lowercase(),
        })
        .collect()
}

/// Percent-decoding, plus `+` for space.
///
/// `+` is not in RFC 6068 — it is a form-encoding convention — but enough real
/// links use it that decoding it is the pragmatic choice. A literal plus in an
/// address is percent-encoded by anything that generates these correctly, so
/// `first+tag@example.com` written as `first%2Btag@example.com` survives.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // Two hex digits, read as bytes. Not a slice of the text: the link
            // is from whatever page was clicked, and a `%` in front of a
            // multi-byte character would put the cut inside it.
            b'%' => {
                if let (Some(high), Some(low)) = (
                    hex_digit(bytes.get(index + 1)),
                    hex_digit(bytes.get(index + 2)),
                ) {
                    out.push((high << 4) | low);
                    index += 3;
                } else {
                    // A stray `%` is far more likely to be a literal than a
                    // truncated escape; dropping it would silently mangle a
                    // subject line.
                    out.push(b'%');
                    index += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy_owned(out)
}

/// One hex digit's value. Digits only — `from_str_radix` would also take a
/// leading sign.
fn hex_digit(byte: Option<&u8>) -> Option<u8> {
    char::from(*byte?)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Mailbox {
        Mailbox {
            name: Some("Me".into()),
            address: "me@example.com".into(),
        }
    }

    #[test]
    fn a_bare_address_opens_a_composer_addressed_to_it() {
        let draft = prefill("mailto:ada@example.com", me()).expect("a mailto");
        assert_eq!(draft.to.len(), 1);
        assert_eq!(draft.to[0].address, "ada@example.com");
        assert_eq!(draft.from.address, "me@example.com");
    }

    #[test]
    fn subject_and_body_are_percent_decoded() {
        let draft = prefill(
            "mailto:ada@example.com?subject=Release%20plan&body=Line%20one%0ALine%20two",
            me(),
        )
        .expect("a mailto");
        assert_eq!(draft.subject, "Release plan");
        assert_eq!(draft.body, "Line one\nLine two");
    }

    #[test]
    fn a_bcc_in_a_link_is_never_honoured() {
        // A page that can make the mail client silently blind-copy a third
        // party on a message the user then writes is an attack, and "the field
        // is visible" is not a defence — nobody reads a Bcc they did not fill
        // in.
        let draft = prefill(
            "mailto:ada@example.com?bcc=attacker@example.org&cc=bob@example.net",
            me(),
        )
        .expect("a mailto");
        assert!(
            draft.bcc.is_empty(),
            "a link set a blind copy: {:?}",
            draft.bcc
        );
        assert_eq!(draft.cc[0].address, "bob@example.net");
    }

    #[test]
    fn arbitrary_headers_are_dropped_rather_than_applied() {
        let draft = prefill(
            "mailto:ada@example.com?from=spoofed@example.org&reply-to=x@example.org",
            me(),
        )
        .expect("a mailto");
        assert_eq!(
            draft.from.address, "me@example.com",
            "a link changed who the message is from"
        );
    }

    #[test]
    fn multiple_recipients_are_split_and_folded() {
        let draft =
            prefill("mailto:A@Example.com,b@example.net?to=c@example.org", me()).expect("a mailto");
        let addresses: Vec<&str> = draft.to.iter().map(|m| m.address.as_str()).collect();
        assert_eq!(
            addresses,
            ["a@example.com", "b@example.net", "c@example.org"]
        );
    }

    #[test]
    fn an_empty_mailto_opens_a_blank_composer() {
        // The desktop action, and what a "compose" launcher entry sends.
        let draft = prefill("mailto:", me()).expect("a mailto");
        assert!(draft.to.is_empty());
        assert_eq!(draft.problem(), Some("this draft has no recipients"));
    }

    #[test]
    fn something_that_is_not_a_mailto_is_refused() {
        assert!(prefill("https://example.com", me()).is_none());
        assert!(prefill("ada@example.com", me()).is_none());
    }

    #[test]
    fn a_broken_escape_does_not_eat_the_rest_of_the_field() {
        // A stray % is far more likely a literal than a truncated escape.
        let draft =
            prefill("mailto:a@example.com?subject=100%25%20or%20nothing", me()).expect("a mailto");
        assert_eq!(draft.subject, "100% or nothing");
        let draft = prefill("mailto:a@example.com?subject=50%", me()).expect("a mailto");
        assert_eq!(draft.subject, "50%");
    }

    #[test]
    fn a_stray_percent_before_a_multibyte_character_is_kept_rather_than_crashing() {
        // The link comes from whatever page was clicked. A `%` whose next two
        // bytes split a character must read as the literal it is — slicing
        // the text there would panic the running mail client.
        let draft =
            prefill("mailto:a@example.com?subject=%a\u{e9}t\u{e9}", me()).expect("a mailto");
        assert_eq!(draft.subject, "%a\u{e9}t\u{e9}");
        let draft = prefill("mailto:a@example.com?subject=%\u{e9}", me()).expect("a mailto");
        assert_eq!(draft.subject, "%\u{e9}");
    }

    #[test]
    fn escapes_that_are_not_utf8_become_replacement_characters() {
        // Nothing obliges a link to decode to text. The bytes that do not are
        // marked rather than dropped, and the valid text around them is kept.
        let draft =
            prefill("mailto:a@example.com?subject=caf%E9%20%C3%A9", me()).expect("a mailto");
        assert_eq!(draft.subject, "caf\u{fffd} \u{e9}");
    }

    #[test]
    fn an_escape_is_two_hex_digits_and_nothing_else() {
        // A sign is not a digit: `%+1` is a stray percent, then the `+` that
        // means a space, then a one — not a control character.
        let draft = prefill("mailto:a@example.com?subject=%+1", me()).expect("a mailto");
        assert_eq!(draft.subject, "% 1");
    }

    #[test]
    fn the_scheme_is_recognised_whatever_its_case() {
        // RFC 3986 schemes are case-insensitive, and the launch path already
        // accepts `Mailto:` — refusing it here opened nothing for the click.
        let draft = prefill("Mailto:ada@example.com", me()).expect("a mailto");
        assert_eq!(draft.to[0].address, "ada@example.com");
    }

    #[test]
    fn a_plus_in_an_address_survives_when_it_was_encoded_properly() {
        let draft = prefill("mailto:first%2Btag@example.com", me()).expect("a mailto");
        assert_eq!(draft.to[0].address, "first+tag@example.com");
    }
}
