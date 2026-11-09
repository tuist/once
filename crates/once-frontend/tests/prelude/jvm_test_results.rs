use std::process::Command;

use super::{all_prelude_source, eval_prelude_string_function_in};

fn compile_suite(dir: &tempfile::TempDir) {
    let runner =
        eval_prelude_string_function_in(all_prelude_source(), "_jvm_test_runner_source", "()")
            .unwrap();
    std::fs::write(dir.path().join("OnceJvmTestRunner.java"), runner).unwrap();
    std::fs::write(
        dir.path().join("Ignore.java"),
        "package org.junit;\n@java.lang.annotation.Retention(java.lang.annotation.RetentionPolicy.RUNTIME)\n@java.lang.annotation.Target({java.lang.annotation.ElementType.TYPE, java.lang.annotation.ElementType.METHOD})\npublic @interface Ignore {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("BaseSuite.java"),
        r#"public abstract class BaseSuite {
  public void testInherited() {
    if (!(this instanceof Suite)) throw new AssertionError("wrong inherited receiver");
  }
  public void testOverridden() { throw new AssertionError("shadowed base test executed"); }
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Suite.java"),
        r#"public class Suite extends BaseSuite {
  public void testTopLevel() {}
  private void testPrivate() {}
  @Override public void testOverridden() {}
  @org.junit.Ignore
  public void testIgnoredMethod() { throw new AssertionError("ignored method executed"); }
  @org.junit.Ignore
  public static class IgnoredSuite {
    static { if (true) throw new AssertionError("ignored suite initialized"); }
    public void testIgnoredSuite() { throw new AssertionError("ignored suite executed"); }
  }
  public static class Nested {
    public void testPasses() {}
    public static class Deep {
      public void testFails() { throw new AssertionError("nested failure"); }
    }
  }
  public class Inner {
    public void testNotIndependent() { throw new AssertionError("inner helper"); }
  }
  public abstract static class AbstractHelper {
    public void testNotIndependent() { throw new AssertionError("abstract helper"); }
  }
  public static class Helper {
    static { if (true) throw new AssertionError("helper initialized"); }
  }
  public Object anonymous = new Object() {
    public void testNotIndependent() { throw new AssertionError("anonymous helper"); }
  };
  public Object local() {
    class Local {
      public void testNotIndependent() { throw new AssertionError("local helper"); }
    }
    return new Local();
  }
}
"#,
    )
    .unwrap();
    let compile = Command::new("javac")
        .args([
            "-d",
            "classes",
            "OnceJvmTestRunner.java",
            "Ignore.java",
            "BaseSuite.java",
            "Suite.java",
        ])
        .current_dir(dir.path())
        .output()
        .expect("JDK required for JVM runner regression");
    assert!(compile.status.success(), "{compile:?}");
}

#[test]
fn jvm_runner_discovers_static_nested_suites_and_propagates_their_failures() {
    let dir = tempfile::tempdir().unwrap();
    compile_suite(&dir);
    for runner_type in ["kotlin_jvm", "android_local"] {
        let output = Command::new("java")
            .args([
                "-cp",
                "classes",
                "OnceJvmTestRunner",
                "classes",
                "results.json",
                "test.log",
                "native.txt",
                "Tests",
                runner_type,
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("results.json")).unwrap())
                .unwrap();
        assert_eq!(report["summary"]["total"], 8, "{report}");
        assert_eq!(report["summary"]["passed"], 5, "{report}");
        assert_eq!(report["summary"]["failed"], 1, "{report}");
        assert_eq!(report["summary"]["skipped"], 2, "{report}");
        assert!(
            report["cases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|case| { case["suite"] == "Suite$Nested$Deep" && case["status"] == "failed" }),
            "{report}"
        );
        let filtered = Command::new("java")
            .args([
                "-cp",
                "classes",
                "OnceJvmTestRunner",
                "classes",
                "filtered.json",
                "filtered.log",
                "filtered.txt",
                "Tests",
                runner_type,
                "Suite$Nested#testPasses",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(filtered.status.success(), "{filtered:?}");
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("filtered.json")).unwrap())
                .unwrap();
        assert_eq!(report["summary"]["total"], 1, "{report}");
        assert_eq!(report["summary"]["passed"], 1, "{report}");
    }
}
