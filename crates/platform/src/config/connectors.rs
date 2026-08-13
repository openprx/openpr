//! Connector delivery credentials, partitioned by the workspace that owns them.
//!
//! # Why the partition is the security boundary
//!
//! A connector endpoint and its `auth_policy` are attacker controlled by design: anyone can
//! create a workspace and is its admin. The previous design provisioned credentials as process
//! environment variables and defended the tenant boundary by *encoding the workspace into the
//! variable name*
//! (`OPENPR_CONNECTOR_SECRET_W_<workspace uuid, no dashes, uppercase>_<NAME>`), then string
//! matching a reference against the prefix of the reading workspace. That worked, but it defended
//! a flat, process wide namespace with a string comparison: every credential of every tenant was
//! reachable from one `std::env::var` call, and only the prefix check stood between them.
//!
//! The file backed store removes the flat namespace instead of guarding it. Credentials live in
//! a two level map keyed first by workspace id, and [`ConnectorSecrets::get`] takes the workspace
//! id of the connector performing the read as its first argument. A lookup therefore cannot
//! traverse into another tenant's map: there is no name, however crafted, that reaches outside
//! the sub-map selected by `workspace_id`. Name shape rules and the reserved name list below are
//! kept as a second, independent gate, not as the boundary itself.
//!
//! The workspace id passed in must always be the one read from the connector row (or from the
//! authenticated route path), never a value taken from a request body. That id is a server
//! generated v4 UUID and is not a mutable column, so a workspace an attacker owns can only ever
//! select its own sub-map. Knowing a victim's workspace id does not help either: the id used for
//! the lookup is the reader's, not one the reference names.

use std::collections::BTreeMap;
use std::fmt;

use uuid::Uuid;

use super::secret::Secret;

/// Longest accepted credential name. Keeps diagnostics bounded.
pub const MAX_CONNECTOR_SECRET_NAME_LEN: usize = 128;

/// Credential names that must never be usable, even inside a workspace's own namespace.
///
/// The partitioned store already makes platform secrets unreachable, because platform secrets are
/// not in the store at all. This list keeps holding if a future code path ever widens the lookup,
/// and it makes a copy-pasted platform variable name fail loudly at load time instead of
/// resolving to a workspace credential that merely shares its name.
const RESERVED_NAMES: &[&str] = &["JWT_SECRET", "DATABASE_URL", "RUST_LOG"];

/// Credential name prefixes that must never be usable, for the same reason as [`RESERVED_NAMES`].
///
/// `OPENPR_` also rejects a name copied verbatim from the retired
/// `OPENPR_CONNECTOR_SECRET_W_<uuid>_<NAME>` environment scheme, so a half finished migration
/// cannot leave a reference that looks namespaced but is matched as a plain name.
const RESERVED_PREFIXES: &[&str] = &["POSTGRES_", "PG", "AWS_", "OPENPR_"];

/// Why a connector credential lookup or declaration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorSecretError {
    /// The name does not match `[A-Z_][A-Z0-9_]*` within the length limit.
    InvalidName { name: String },
    /// The name is reserved by the platform.
    ReservedName { name: String },
    /// This workspace has no credential under that name.
    ///
    /// Deliberately the same error whether the workspace has no credentials at all or merely
    /// lacks this one, and it never reports that another workspace does have it: the lookup never
    /// consults another workspace, so no cross-tenant existence oracle exists.
    NotProvisioned { workspace_id: Uuid, name: String },
}

impl fmt::Display for ConnectorSecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(
                f,
                "connector credential name {name} is invalid, expected [A-Z_][A-Z0-9_]* of at most \
                 {MAX_CONNECTOR_SECRET_NAME_LEN} characters"
            ),
            Self::ReservedName { name } => write!(f, "connector credential name {name} is reserved by the platform"),
            Self::NotProvisioned { workspace_id, name } => write!(
                f,
                "no connector credential named {name} is provisioned for workspace {workspace_id}; add it under \
                 [connectors.secrets.\"{workspace_id}\"] in the configuration file. A connector can only read \
                 credentials provisioned for the workspace that owns it"
            ),
        }
    }
}

impl std::error::Error for ConnectorSecretError {}

/// Checks a connector credential name against the shape and reserved word rules.
///
/// Exposed so the API can reject a bad `secret_ref` when the connector is written, rather than
/// only when a delivery is attempted hours later.
pub fn validate_connector_secret_name(name: &str) -> Result<(), ConnectorSecretError> {
    let shape_ok = !name.is_empty()
        && name.len() <= MAX_CONNECTOR_SECRET_NAME_LEN
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !shape_ok {
        return Err(ConnectorSecretError::InvalidName { name: name.to_string() });
    }
    if RESERVED_NAMES.contains(&name) || RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
        return Err(ConnectorSecretError::ReservedName { name: name.to_string() });
    }
    Ok(())
}

/// Connector delivery credentials, partitioned by owning workspace.
///
/// See the module documentation for why the partition, and not a name convention, is what keeps
/// one tenant out of another tenant's credentials.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConnectorSecrets {
    by_workspace: BTreeMap<Uuid, BTreeMap<String, Secret>>,
}

impl ConnectorSecrets {
    /// Builds the store from already validated entries.
    pub const fn new(by_workspace: BTreeMap<Uuid, BTreeMap<String, Secret>>) -> Self {
        Self { by_workspace }
    }

    /// Reads one credential on behalf of a connector owned by `workspace_id`.
    ///
    /// `workspace_id` must come from the connector row or the authenticated route path. The
    /// workspace sub-map is selected first and the name is only ever looked up inside it, so a
    /// reference cannot name a credential belonging to another tenant.
    pub fn get(&self, workspace_id: Uuid, name: &str) -> Result<&Secret, ConnectorSecretError> {
        validate_connector_secret_name(name)?;
        self.by_workspace
            .get(&workspace_id)
            .and_then(|owned| owned.get(name))
            .ok_or_else(|| ConnectorSecretError::NotProvisioned {
                workspace_id,
                name: name.to_string(),
            })
    }

    /// Whether any credential at all is provisioned for a workspace.
    pub fn has_workspace(&self, workspace_id: Uuid) -> bool {
        self.by_workspace.contains_key(&workspace_id)
    }

    /// Credential names provisioned for one workspace, for operator facing diagnostics.
    ///
    /// Only ever names, never values, and only ever for the workspace asked about.
    pub fn names_for(&self, workspace_id: Uuid) -> impl Iterator<Item = &str> {
        self.by_workspace
            .get(&workspace_id)
            .into_iter()
            .flat_map(|owned| owned.keys().map(String::as_str))
    }

    /// Number of workspaces that carry at least one credential.
    pub fn workspace_count(&self) -> usize {
        self.by_workspace.len()
    }

    /// Total number of provisioned credentials across all workspaces.
    pub fn entry_count(&self) -> usize {
        self.by_workspace.values().map(BTreeMap::len).sum()
    }
}

impl fmt::Debug for ConnectorSecrets {
    /// Prints shape only. Credential names identify a tenant's integrations, so not even the keys
    /// are rendered.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorSecrets")
            .field("workspaces", &self.workspace_count())
            .field("entries", &self.entry_count())
            .finish()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{ConnectorSecretError, ConnectorSecrets, Secret, validate_connector_secret_name};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn store(entries: &[(Uuid, &str, &str)]) -> ConnectorSecrets {
        let mut by_workspace: BTreeMap<Uuid, BTreeMap<String, Secret>> = BTreeMap::new();
        for (workspace, name, value) in entries {
            by_workspace
                .entry(*workspace)
                .or_default()
                .insert((*name).to_string(), Secret::new(*value));
        }
        ConnectorSecrets::new(by_workspace)
    }

    #[test]
    fn a_workspace_reads_its_own_credential() {
        let workspace = Uuid::new_v4();
        let secrets = store(&[(workspace, "SHIPPING", "shipping-value")]);
        let found = secrets.get(workspace, "SHIPPING").expect("own credential is readable");
        assert_eq!(found.expose(), "shipping-value");
    }

    #[test]
    fn a_workspace_cannot_read_another_workspaces_credential() {
        let victim = Uuid::new_v4();
        let attacker = Uuid::new_v4();
        let secrets = store(&[
            (victim, "PAYMENTS", "victim-value"),
            (attacker, "OWN", "attacker-value"),
        ]);

        let error = secrets
            .get(attacker, "PAYMENTS")
            .expect_err("a name owned by another tenant must not resolve");
        assert_eq!(
            error,
            ConnectorSecretError::NotProvisioned {
                workspace_id: attacker,
                name: "PAYMENTS".to_string(),
            }
        );
        assert!(!error.to_string().contains("victim-value"));
    }

    #[test]
    fn the_retired_environment_namespace_cannot_be_smuggled_into_a_name() {
        let victim = Uuid::new_v4();
        let attacker = Uuid::new_v4();
        let secrets = store(&[(victim, "PAYMENTS", "victim-value")]);

        // The exact shape the retired environment scheme used, aimed at the victim.
        let legacy = format!(
            "OPENPR_CONNECTOR_SECRET_W_{}_PAYMENTS",
            victim.simple().to_string().to_ascii_uppercase()
        );
        let error = secrets
            .get(attacker, &legacy)
            .expect_err("a legacy namespaced name must be refused");
        assert!(matches!(error, ConnectorSecretError::ReservedName { .. }), "{error:?}");

        // And it does not resolve for the victim either: the store holds plain names.
        assert!(secrets.get(victim, &legacy).is_err());
    }

    #[test]
    fn an_unprovisioned_workspace_gets_the_same_error_as_a_missing_name() {
        let provisioned = Uuid::new_v4();
        let empty = Uuid::new_v4();
        let secrets = store(&[(provisioned, "SHIPPING", "value")]);

        let missing_name = secrets.get(provisioned, "ABSENT").expect_err("missing name");
        let missing_workspace = secrets.get(empty, "SHIPPING").expect_err("missing workspace");
        assert!(matches!(missing_name, ConnectorSecretError::NotProvisioned { .. }));
        assert!(matches!(missing_workspace, ConnectorSecretError::NotProvisioned { .. }));
    }

    #[test]
    fn platform_secret_names_are_reserved() {
        for name in [
            "JWT_SECRET",
            "DATABASE_URL",
            "RUST_LOG",
            "AWS_SECRET_ACCESS_KEY",
            "PGPASSWORD",
            "POSTGRES_PASSWORD",
        ] {
            let error = validate_connector_secret_name(name).expect_err("reserved name must be refused");
            assert!(matches!(error, ConnectorSecretError::ReservedName { .. }), "{name}");
        }
    }

    #[test]
    fn malformed_names_are_refused() {
        for name in [
            "",
            "lowercase",
            "1LEADING_DIGIT",
            "HAS-DASH",
            "HAS SPACE",
            "HAS.DOT",
            "TRAILING\n",
        ] {
            let error = validate_connector_secret_name(name).expect_err("malformed name must be refused");
            assert!(matches!(error, ConnectorSecretError::InvalidName { .. }), "{name:?}");
        }
        let too_long = "A".repeat(super::MAX_CONNECTOR_SECRET_NAME_LEN + 1);
        assert!(matches!(
            validate_connector_secret_name(&too_long),
            Err(ConnectorSecretError::InvalidName { .. })
        ));
    }

    #[test]
    fn well_formed_names_are_accepted() {
        for name in ["A", "SHIPPING", "SHIPPING_V2", "_INTERNAL", "X9"] {
            validate_connector_secret_name(name).expect("well formed name should be accepted");
        }
    }

    #[test]
    fn debug_output_reveals_neither_values_nor_names() {
        let workspace = Uuid::new_v4();
        let secrets = store(&[(workspace, "SHIPPING", "shipping-value")]);
        let rendered = format!("{secrets:?}");
        assert!(!rendered.contains("shipping-value"), "{rendered}");
        assert!(!rendered.contains("SHIPPING"), "{rendered}");
        assert!(rendered.contains("workspaces: 1"), "{rendered}");
        assert!(rendered.contains("entries: 1"), "{rendered}");
    }

    #[test]
    fn diagnostics_only_describe_the_workspace_asked_about() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let secrets = store(&[(first, "ONE", "1"), (second, "TWO", "2")]);
        let names: Vec<&str> = secrets.names_for(first).collect();
        assert_eq!(names, vec!["ONE"]);
        assert!(secrets.has_workspace(second));
        assert!(!secrets.has_workspace(Uuid::new_v4()));
        assert_eq!(secrets.workspace_count(), 2);
        assert_eq!(secrets.entry_count(), 2);
    }
}
