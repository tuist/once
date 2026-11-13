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

fn compile_junit4_suite(dir: &tempfile::TempDir, classpath: &str) {
    let runner = eval_prelude_string_function_in(
        all_prelude_source(),
        "_jvm_test_runner_source",
        "(junit4 = True)",
    )
    .unwrap();
    std::fs::write(dir.path().join("OnceJvmTestRunner.java"), runner).unwrap();
    std::fs::write(
        dir.path().join("LifecycleSuite.java"),
        r#"import org.junit.*;
public class LifecycleSuite {
  static int executions;
  boolean ready;
  boolean ruleReady;
  @BeforeClass public static void beforeClass() { executions = 0; }
  @AfterClass public static void afterClass() { Assert.assertEquals(3, executions); }
  @Before public void before() { Assert.assertTrue(ruleReady); ready = true; }
  @After public void after() { Assert.assertTrue(ready); executions++; }
  @Rule public org.junit.rules.ExternalResource resource = new org.junit.rules.ExternalResource() {
    protected void before() { ruleReady = true; }
    protected void after() { Assert.assertTrue(ready); }
  };
  @Test public void works() { Assert.assertTrue(ready); }
  @Test(expected = IllegalArgumentException.class) public void expected() {
    Assert.assertTrue(ready);
    throw new IllegalArgumentException("expected");
  }
  @Test public void assumption() { Assume.assumeTrue(false); }
  @Ignore @Test public void ignored() { throw new AssertionError("ignored"); }
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("ParameterSuite.java"),
        r"import org.junit.*;
@org.junit.runner.RunWith(org.junit.runners.Parameterized.class)
public class ParameterSuite {
  @org.junit.runners.Parameterized.Parameters public static Object[] values() {
    return new Object[] {3, 7};
  }
  final int value;
  public ParameterSuite(int value) { this.value = value; }
  @Test public void parameter() { Assert.assertEquals(3, value); }
}
",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("FailureSuite.java"),
        r#"import org.junit.*;
public class FailureSuite {
  @After public void after() { throw new AssertionError("teardown failed"); }
  @Test public void bodyPasses() {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("BrokenSuite.java"),
        r#"import org.junit.*;
public class BrokenSuite {
  @BeforeClass public static void before() { throw new AssertionError("setup failed"); }
  @Test public void unreachable() {}
}
"#,
    )
    .unwrap();
    write_junit4_discovery_fixtures(dir);
    let compile = Command::new("javac")
        .args(["-classpath", classpath, "-d", "classes"])
        .args([
            "OnceJvmTestRunner.java",
            "LifecycleSuite.java",
            "ParameterSuite.java",
            "FailureSuite.java",
            "BrokenSuite.java",
            "ASuite.java",
            "ZOnceTest.java",
            "InvalidTest.java",
            "InheritedInvalidTest.java",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(compile.status.success(), "{compile:?}");
}

fn write_junit4_discovery_fixtures(dir: &tempfile::TempDir) {
    for (name, source) in [
        (
            "ASuite.java",
            r"@org.junit.runner.RunWith(org.junit.runners.Suite.class)
@org.junit.runners.Suite.SuiteClasses({ZOnceTest.class})
public class ASuite {}
",
        ),
        (
            "ZOnceTest.java",
            r#"public class ZOnceTest {
  static int runs;
  @org.junit.Test public void once() {
    if (++runs == 1) throw new AssertionError("first execution fails");
    throw new AssertionError("duplicate execution");
  }
}
"#,
        ),
        (
            "InvalidTest.java",
            r"public class InvalidTest {
  @org.junit.Test private void hidden() {}
}
",
        ),
        (
            "InheritedInvalidTest.java",
            r"abstract class InvalidBase {
  @org.junit.Test protected void hidden() {}
}
public class InheritedInvalidTest extends InvalidBase {}
",
        ),
    ] {
        std::fs::write(dir.path().join(name), source).unwrap();
    }
}

#[test]
#[ignore = "requires JUNIT4_CLASSPATH containing JUnit 4 and Hamcrest jars"]
fn junit4_runner_preserves_native_lifecycle_rules_parameters_and_failures() {
    let classpath = std::env::var("JUNIT4_CLASSPATH").unwrap();
    let dir = tempfile::tempdir().unwrap();
    compile_junit4_suite(&dir, &classpath);
    let runtime_classpath = std::env::join_paths(
        [dir.path().join("classes")]
            .into_iter()
            .chain(std::env::split_paths(&classpath)),
    )
    .unwrap();
    for (filters, total, passed, failed, skipped) in [
        (vec![], 11, 3, 6, 2),
        (vec!["ASuite"], 1, 0, 1, 0),
        (vec!["ZOnceTest"], 1, 0, 1, 0),
        (vec!["InvalidTest"], 1, 0, 1, 0),
        (vec!["InheritedInvalidTest"], 1, 0, 1, 0),
        (vec!["LifecycleSuite"], 4, 2, 0, 2),
        (vec!["ParameterSuite#parameter"], 2, 1, 1, 0),
        (vec!["ParameterSuite#parameter[0]"], 1, 1, 0, 0),
        (vec!["ParameterSuite#missing"], 1, 0, 1, 0),
    ] {
        let output = Command::new("java")
            .arg("-cp")
            .arg(&runtime_classpath)
            .args([
                "OnceJvmTestRunner",
                "classes",
                "results.json",
                "run.log",
                "native.txt",
                "Tests",
                "kotlin_jvm",
            ])
            .args(filters)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(output.status.success(), failed == 0, "{output:?}");
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("results.json")).unwrap())
                .unwrap();
        assert_eq!(report["summary"]["total"], total, "{report}");
        assert_eq!(report["summary"]["passed"], passed, "{report}");
        assert_eq!(report["summary"]["failed"], failed, "{report}");
        assert_eq!(report["summary"]["skipped"], skipped, "{report}");
        if total == 11 {
            let log = std::fs::read_to_string(dir.path().join("run.log")).unwrap();
            assert!(log.contains("setup failed"), "{log}");
            assert!(log.contains("teardown failed"), "{log}");
            assert!(log.contains("first execution fails"), "{log}");
            assert!(!log.contains("duplicate execution"), "{log}");
        }
    }
}
