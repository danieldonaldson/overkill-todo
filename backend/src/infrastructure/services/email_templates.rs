//! Transactional email templates.
//!
//! Each builder returns a [`RenderedEmail`] carrying the subject, an HTML body,
//! and a plaintext alternative so the SMTP transport can send a proper
//! `multipart/alternative` message. User-supplied values (names) are
//! HTML-escaped before interpolation into the HTML body.

use std::fmt::Write as _;

/// A fully-rendered email: subject plus both MIME alternatives.
pub struct RenderedEmail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

/// Minimal HTML-entity escaping for values interpolated into the HTML body.
///
/// Names are user-controlled (e.g. `"O'Brien"`, or a malicious `"<script>"`),
/// so they must never reach the HTML body unescaped. The 6-digit codes and the
/// magic-link URL are system-generated and not escaped here.
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A friendly greeting line. Falls back to "Hi there," when no name is known.
fn greeting(first_name: &str) -> String {
    let name = first_name.trim();
    if name.is_empty() {
        "Hi there,".to_string()
    } else {
        format!("Hi {},", escape_html(name))
    }
}

/// Wraps body markup in a minimal, client-safe HTML document shell.
fn html_shell(app_name: &str, inner: &str) -> String {
    let app = escape_html(app_name);
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"></head>\
         <body style=\"margin:0;padding:24px;background:#f4f4f5;\
         font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;color:#18181b;\">\
         <div style=\"max-width:480px;margin:0 auto;background:#ffffff;border-radius:12px;\
         padding:32px;\">{inner}\
         <hr style=\"border:none;border-top:1px solid #e4e4e7;margin:28px 0 16px;\">\
         <p style=\"font-size:12px;color:#71717a;margin:0;\">{app} &middot; This is an \
         automated security message; please do not reply.</p></div></body></html>",
    )
}

/// Verification email: 6-digit code plus, when available, a one-click magic link.
#[must_use]
pub fn verification_email(
    app_name: &str,
    first_name: &str,
    code: &str,
    magic_link: Option<&str>,
) -> RenderedEmail {
    let subject = format!("Your {app_name} verification code");
    let greet = greeting(first_name);

    let magic_html = magic_link.map_or_else(String::new, |link| {
        format!(
            "<p style=\"margin:24px 0 8px;\">Or verify in one click:</p>\
             <p style=\"margin:0 0 8px;\"><a href=\"{link}\" \
             style=\"display:inline-block;background:#18181b;color:#ffffff;text-decoration:none;\
             padding:12px 20px;border-radius:8px;font-weight:600;\">Verify my email</a></p>\
             <p style=\"font-size:12px;color:#71717a;margin:0;word-break:break-all;\">\
             If the button doesn't work, paste this link into your browser:<br>{link}</p>",
        )
    });

    let html = html_shell(
        app_name,
        &format!(
            "<p style=\"margin:0 0 16px;\">{greet}</p>\
             <p style=\"margin:0 0 8px;\">Your {app} verification code is:</p>\
             <p style=\"font-size:32px;font-weight:700;letter-spacing:6px;margin:0 0 16px;\">{code}</p>\
             <p style=\"font-size:14px;color:#52525b;margin:0;\">This code expires in 24 hours.</p>\
             {magic_html}",
            app = escape_html(app_name),
        ),
    );

    let mut text = format!(
        "{greet}\n\nYour {app_name} verification code is: {code}\n\nThis code expires in 24 hours.\n",
        greet = strip_greeting_html(&greet),
    );
    if let Some(link) = magic_link {
        let _ = write!(text, "\nOr verify in one click:\n{link}\n");
    }
    text.push_str("\nIf you didn't request this, you can ignore this email.\n");

    RenderedEmail {
        subject,
        html,
        text,
    }
}

/// Password-reset email: 6-digit reset code.
#[must_use]
pub fn password_reset_email(app_name: &str, first_name: &str, code: &str) -> RenderedEmail {
    let subject = format!("Your {app_name} password reset code");
    let greet = greeting(first_name);

    let html = html_shell(
        app_name,
        &format!(
            "<p style=\"margin:0 0 16px;\">{greet}</p>\
             <p style=\"margin:0 0 8px;\">Your {app} password reset code is:</p>\
             <p style=\"font-size:32px;font-weight:700;letter-spacing:6px;margin:0 0 16px;\">{code}</p>\
             <p style=\"font-size:14px;color:#52525b;margin:0;\">This code expires in 24 hours. \
             If you did not request a reset, you can safely ignore this email.</p>",
            app = escape_html(app_name),
        ),
    );

    let text = format!(
        "{greet}\n\nYour {app_name} password reset code is: {code}\n\nThis code expires in 24 hours.\n\n\
         If you did not request a reset, you can safely ignore this email.\n",
        greet = strip_greeting_html(&greet),
    );

    RenderedEmail {
        subject,
        html,
        text,
    }
}

/// The greeting is built once for HTML (escaped); for the plaintext alternative
/// we want the raw name back. Names only ever contain the entities introduced by
/// [`escape_html`], so reverse exactly those.
fn strip_greeting_html(greet: &str) -> String {
    greet
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_special_chars_in_name() {
        let email = verification_email("App", "<script>alert(1)</script>", "123456", None);
        assert!(!email.html.contains("<script>"));
        assert!(email.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn plaintext_restores_raw_name() {
        let email = password_reset_email("App", "User & Admin", "654321");
        assert!(email.text.contains("Hi User & Admin,"));
        assert!(!email.text.contains("&amp;"));
    }

    #[test]
    fn empty_name_falls_back_to_generic_greeting() {
        let email = verification_email("App", "   ", "111111", None);
        assert!(email.html.contains("Hi there,"));
        assert!(email.text.contains("Hi there,"));
    }

    #[test]
    fn magic_link_included_only_when_present() {
        let with = verification_email("App", "Jo", "111111", Some("https://x/verify?token=abc"));
        assert!(with.html.contains("https://x/verify?token=abc"));
        assert!(with.text.contains("https://x/verify?token=abc"));

        let without = verification_email("App", "Jo", "111111", None);
        assert!(!without.html.contains("Verify my email"));
        assert!(!without.text.contains("one click"));
    }

    #[test]
    fn code_and_expiry_present_in_both_parts() {
        let email = verification_email("App", "Jo", "246813", None);
        assert!(email.html.contains("246813"));
        assert!(email.text.contains("246813"));
        assert!(email.html.contains("24 hours"));
        assert!(email.text.contains("24 hours"));
    }

    #[test]
    fn escape_html_covers_all_special_chars() {
        assert_eq!(escape_html("&<>\"' x"), "&amp;&lt;&gt;&quot;&#x27; x");
    }

    #[test]
    fn app_name_is_escaped_in_html_shell_and_subject() {
        let email = verification_email("Ben & Jerry's <Co>", "Jo", "111111", None);
        // App name appears escaped in the HTML body/footer.
        assert!(email.html.contains("Ben &amp; Jerry&#x27;s &lt;Co&gt;"));
        assert!(!email.html.contains("Ben & Jerry's <Co>"));
        // Subject is plaintext and not escaped.
        assert_eq!(email.subject, "Your Ben & Jerry's <Co> verification code");
    }

    #[test]
    fn password_reset_subject_and_both_parts_carry_code() {
        let email = password_reset_email("App", "Jo", "778899");
        assert_eq!(email.subject, "Your App password reset code");
        assert!(email.html.contains("778899"));
        assert!(email.text.contains("778899"));
        assert!(email.html.contains("password reset code"));
        assert!(email.text.contains("safely ignore this email"));
    }

    #[test]
    fn verification_text_includes_ignore_notice() {
        let email = verification_email("App", "Jo", "111111", None);
        assert!(
            email
                .text
                .contains("If you didn't request this, you can ignore this email.")
        );
    }
}
