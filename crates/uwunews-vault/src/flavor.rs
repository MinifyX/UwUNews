//! Whose account a vault belongs to.
//!
//! UwUNews has no account of its own: it joins the one UwUSSH or UwURDP
//! already syncs through, so a feed list follows the same devices as the
//! hosts. The two apps derive their keys alike, but every label that goes into
//! a derivation or a seal starts with the app's name — `uwussh/…` or
//! `uwurdp/…` — so a vault one of them created only opens with that app's
//! labels. The flavor is which set this vault uses. It is chosen when the
//! account is created or joined, kept with the vault, and never guessed later.
//!
//! Two labels are the same for both and stay outside of this: the server login
//! (`uwussh/server-auth/v1`) and the session proof (`uwussh/session/v1`) are
//! what UwUSync-Server checks, and it only knows UwUSSH's.

/// The label set of the app that created the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Flavor {
    /// An account UwUSSH created — and the one UwUNews creates itself, so a
    /// UwUSSH device can join it later.
    #[default]
    Ssh,
    /// An account UwURDP created.
    Rdp,
}

impl Flavor {
    pub const ALL: [Self; 2] = [Self::Ssh, Self::Rdp];

    /// What the labels start with.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Ssh => "uwussh",
            Self::Rdp => "uwurdp",
        }
    }

    /// How it is written down: `ssh` or `rdp`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Rdp => "rdp",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ssh" => Some(Self::Ssh),
            "rdp" => Some(Self::Rdp),
            _ => None,
        }
    }

    /// `prefix/rest`, as bytes: `Flavor::Rdp.label("record/v2")` is
    /// `uwurdp/record/v2`.
    pub fn label(self, rest: &str) -> Vec<u8> {
        format!("{}/{rest}", self.prefix()).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_the_other_apps_labels() {
        assert_eq!(Flavor::Ssh.label("record/v2"), b"uwussh/record/v2");
        assert_eq!(Flavor::Rdp.label("vault-key/v1"), b"uwurdp/vault-key/v1");
        for flavor in Flavor::ALL {
            assert_eq!(Flavor::parse(flavor.as_str()), Some(flavor));
        }
        assert_eq!(Flavor::parse("news"), None);
        assert_eq!(Flavor::default(), Flavor::Ssh);
    }
}
