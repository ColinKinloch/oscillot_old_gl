use std::process::Command;

static RES_SPEC: &'static str = "oscillot.gresource.xml";
static RES_DIR: &'static str = "src/resources";

fn main() {
  let mut gcr = Command::new("glib-compile-resources");
   gcr.args(&["--generate", RES_SPEC])
   .current_dir(RES_DIR)
   .status()
   .unwrap();
  let deps = gcr.args(&["--generate-dependencies", RES_SPEC])
    .current_dir(RES_DIR)
    .output()
    .unwrap();
  println!("cargo:rerun-if-changed=./src/resources/*")
}
