use vergen::{BuildBuilder, Emitter};
use vergen_gitcl::GitclBuilder;

fn main() {
    vergen().expect("Failed to run vergen");
}

fn vergen() -> anyhow::Result<()> {
    let git = GitclBuilder::default().sha(false).build()?;

    Emitter::default().add_instructions(&git)?.emit()?;

    Ok(())
}
