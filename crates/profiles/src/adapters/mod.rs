mod codex;

use anyhow::Result;
use canban_types::Profile;

use crate::launch::LaunchSpec;

pub(crate) fn prepare(profile: Option<&Profile>, spec: &mut LaunchSpec) -> Result<()> {
    // Select the declared engine, never a role ID or executable basename.
    // Herdr templates and untyped custom commands retain their own contract.
    if profile.and_then(|p| p.preset.as_deref()) == Some("codex") {
        codex::prepare(spec)?;
    }
    // Other engines and ordinary shells are pass-through.
    Ok(())
}
