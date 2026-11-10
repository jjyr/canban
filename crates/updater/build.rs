fn main() {
    println!("cargo:rerun-if-env-changed=CANBAN_RELEASE_VERSION");
    println!("cargo:rerun-if-env-changed=CANBAN_RELEASE_COMMIT");
}
