def attr(name, ty, required = False, default = None, docs = "", configurable = True):
    return {
        "name": name,
        "ty": ty,
        "required": required,
        "default": default,
        "docs": docs,
        "configurable": configurable,
    }

def dep(name, expected_providers, docs = ""):
    return {
        "name": name,
        "expected_providers": expected_providers,
        "docs": docs,
    }

def capability(name, output_groups, requires_outputs = []):
    return {
        "name": name,
        "output_groups": output_groups,
        "requires_outputs": requires_outputs,
    }

def tool(name, executables = []):
    return {
        "name": name,
        "executables": executables or [name],
    }

def example(slug, name, use_when, path = None):
    return {
        "_once_example": True,
        "slug": slug,
        "name": name,
        "use_when": use_when,
        "path": path or ("examples/" + slug),
    }

def source_reference(system, symbol, url, use_when, content_digest = None):
    return {
        "_once_source_reference": True,
        "system": system,
        "symbol": symbol,
        "url": url,
        "use_when": use_when,
        "content_digest": content_digest,
    }

def target_kind(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, tools = [], source_references = []):
    return {
        "_once_target_kind": True,
        "kind": kind,
        "docs": docs,
        "attrs": attrs,
        "deps": deps,
        "providers": providers,
        "capabilities": capabilities,
        "tools": tools,
        "examples": examples,
        "source_references": source_references,
        "impl": impl,
    }

def rule(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, tools = [], source_references = []):
    return target_kind(
        kind = kind,
        docs = docs,
        attrs = attrs,
        deps = deps,
        providers = providers,
        capabilities = capabilities,
        examples = examples,
        impl = impl,
        tools = tools,
        source_references = source_references,
    )

def _ends_with(value, suffix):
    if len(value) < len(suffix):
        return False
    return value[len(value) - len(suffix):] == suffix

def _filter_by_extensions(paths, extensions):
    out = []
    for path in paths:
        for ext in extensions:
            if _ends_with(path, ext):
                out.append(path)
                break
    return out

def _file_globs(patterns):
    expanded = []
    for pattern in patterns:
        expanded.append(pattern)
        if _ends_with(pattern, "/**"):
            expanded.append(pattern + "/*")
    return glob(expanded)

def _package_relative(ctx, path):
    if not path:
        return path
    if path.startswith("/") or path.startswith("."):
        return path
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _resolve_host_executable(requested):
    path_like = "/" in requested or "\\" in requested
    resolved = "" if path_like else host_which_optional(requested)
    if resolved or not requested:
        return resolved
    absolute = requested.startswith("/") or (
        len(requested) > 2 and
        requested[1] == ":" and
        (requested[2] == "/" or requested[2] == "\\")
    )
    candidate = requested if absolute else workspace_root() + "/" + requested
    return candidate if host_file_exists(candidate) else ""

def _apple_materialize_native_dep(ctx, dep, state = None):
    return dep

def _android_materialize_native_dep(ctx, dep, state = None):
    return dep

def _parent_dir(path):
    idx = -1
    for i in range(len(path)):
        if path[i] == "/":
            idx = i
    if idx < 0:
        return ""
    return path[:idx]

def _unique(values):
    seen = {}
    out = []
    for value in values:
        if value not in seen:
            seen[value] = True
            out.append(value)
    return out

def _test_unit_suffix(ctx, unit):
    prefix = ctx["label"]["id"] + "::"
    if not unit.startswith(prefix):
        fail("test unit `" + unit + "` does not belong to target `" + ctx["label"]["id"] + "`")
    return unit[len(prefix):]

def _test_output_dir(ctx):
    batch_id = (ctx.get("test") or {}).get("batch_id")
    if batch_id:
        return ctx["build_dir"] + "/test/batches/" + batch_id
    return ctx["build_dir"] + "/test"

def _basename(path):
    normalized = path.replace("\\", "/")
    parts = normalized.split("/")
    return parts[len(parts) - 1]

def _native_library_key(library):
    return (library.get("abi") or "") + "\x00" + (library.get("path") or "")

def _unique_native_libraries(libraries):
    seen = {}
    out = []
    for library in libraries:
        abi = library.get("abi") or ""
        path = library.get("path") or ""
        if not abi or not path:
            continue
        key = _native_library_key(library)
        if key not in seen:
            seen[key] = True
            out.append({"abi": abi, "path": path})
    return out

def _shell_quote(value):
    if not value:
        return "''"
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _powershell_quote(value):
    return "'" + value.replace("'", "''") + "'"

def _json_string(value):
    out = ["\""]
    for ch in value.elems():
        if ch == "\"":
            out.append("\\\"")
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        else:
            out.append(ch)
    out.append("\"")
    return "".join(out)

def _run_result_json(target_id):
    return "{\"schema\":\"once.run_result.v1\",\"target\":" + _json_string(target_id) + ",\"exit_code\":0}\n"

def _jvm_test_runner_source():
    return """import java.io.PrintWriter;
import java.io.StringWriter;
import java.lang.annotation.Annotation;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Stream;

public final class OnceJvmTestRunner {
  private static final class TestCase {
    String id;
    String name;
    String suite;
    String status;
    String failure;

    TestCase(String id, String name, String suite) {
      this.id = id;
      this.name = name;
      this.suite = suite;
      this.status = "unknown";
      this.failure = "";
    }
  }

  public static void main(String[] args) throws Exception {
    if (args.length < 6) {
      System.err.println("usage: OnceJvmTestRunner <classes> <results> <log> <native-results> <target> <runner-type>");
      System.exit(2);
    }
    Path classes = Paths.get(args[0]);
    Path results = Paths.get(args[1]);
    Path log = Paths.get(args[2]);
    Path nativeResults = Paths.get(args[3]);
    String target = args[4];
    String runnerType = args[5];
    createParent(results);
    createParent(log);
    createParent(nativeResults);

    StringBuilder logText = new StringBuilder();
    List<String> filters = new ArrayList<>();
    for (int i = 6; i < args.length; i++) {
      filters.add(args[i]);
    }
    List<TestCase> cases = runTests(classes, target, filters, logText);
    if (cases.isEmpty()) {
      String reason = filters.isEmpty()
          ? "no test classes found under " + classes
          : "no test matched the filters: " + String.join(", ", filters);
      logText.append(reason).append(System.lineSeparator());
      TestCase testCase = new TestCase(target + "::no-tests", "no-tests", target);
      testCase.status = "failed";
      testCase.failure = reason;
      cases.add(testCase);
    }
    int failed = 0;
    int passed = 0;
    for (TestCase testCase : cases) {
      if ("passed".equals(testCase.status)) {
        passed++;
      } else if ("failed".equals(testCase.status)) {
        failed++;
      }
    }
    String report = reportJson(target, runnerType, cases, passed, failed, log.toString(), nativeResults.toString());
    Files.writeString(log, logText.toString(), StandardCharsets.UTF_8);
    Files.writeString(nativeResults, logText.toString(), StandardCharsets.UTF_8);
    Files.writeString(results, report, StandardCharsets.UTF_8);
    System.exit(failed == 0 ? 0 : 1);
  }

  private static List<TestCase> runTests(Path classes, String target, List<String> filters, StringBuilder logText) throws Exception {
    List<String> classNames = classNames(classes);
    List<TestCase> cases = new ArrayList<>();
    for (String className : classNames) {
      Class<?> type;
      try {
        type = Class.forName(className);
      } catch (Throwable error) {
        TestCase testCase = new TestCase(target + "::" + className, className, className);
        testCase.status = "failed";
        testCase.failure = stackTrace(error);
        cases.add(testCase);
        logText.append(testCase.failure);
        continue;
      }
      Method[] methods = type.getDeclaredMethods();
      List<Method> sortedMethods = new ArrayList<>();
      Collections.addAll(sortedMethods, methods);
      sortedMethods.sort(Comparator.comparing(Method::getName));
      for (Method method : sortedMethods) {
        if (!isTestMethod(method) || !matchesFilter(filters, className, method.getName())) {
          continue;
        }
        TestCase testCase = new TestCase(target + "::" + className + "." + method.getName(), method.getName(), className);
        try {
          invoke(type, method);
          testCase.status = "passed";
          logText.append("passed ").append(className).append(".").append(method.getName()).append(System.lineSeparator());
        } catch (Throwable error) {
          testCase.status = "failed";
          testCase.failure = stackTrace(error);
          logText.append("failed ").append(className).append(".").append(method.getName()).append(System.lineSeparator());
          logText.append(testCase.failure);
        }
        cases.add(testCase);
      }
    }
    return cases;
  }

  private static boolean matchesFilter(List<String> filters, String className, String methodName) {
    if (filters.isEmpty()) {
      return true;
    }
    for (String filter : filters) {
      int separator = filter.indexOf('#');
      String filterClass = separator < 0 ? filter : filter.substring(0, separator);
      String filterMethod = separator < 0 ? "" : filter.substring(separator + 1);
      if ((filterClass.isEmpty() || filterClass.equals(className)) &&
          (filterMethod.isEmpty() || filterMethod.equals(methodName))) {
        return true;
      }
    }
    return false;
  }

  private static List<String> classNames(Path root) throws Exception {
    List<String> names = new ArrayList<>();
    if (!Files.isDirectory(root)) {
      return names;
    }
    try (Stream<Path> stream = Files.walk(root)) {
      stream
          .filter(Files::isRegularFile)
          .map(root::relativize)
          .map(Path::toString)
          .filter(path -> path.endsWith(".class"))
          .filter(path -> !path.contains("$"))
          .filter(path -> !path.endsWith("module-info.class"))
          .filter(path -> !path.endsWith("package-info.class"))
          .map(path -> path.substring(0, path.length() - ".class".length()))
          .map(path -> path.replace('/', '.').replace((char)92, '.'))
          .sorted()
          .forEach(names::add);
    }
    return names;
  }

  private static boolean isTestMethod(Method method) {
    if (method.getParameterCount() != 0) {
      return false;
    }
    if (method.getName().startsWith("test")) {
      return true;
    }
    for (Annotation annotation : method.getDeclaredAnnotations()) {
      String name = annotation.annotationType().getName();
      if ("org.junit.Test".equals(name) || "kotlin.test.Test".equals(name)) {
        return true;
      }
    }
    return false;
  }

  private static void invoke(Class<?> type, Method method) throws Throwable {
    method.setAccessible(true);
    Object receiver = null;
    if (!Modifier.isStatic(method.getModifiers())) {
      receiver = instance(type);
    }
    try {
      method.invoke(receiver);
    } catch (InvocationTargetException error) {
      throw error.getTargetException();
    }
  }

  private static Object instance(Class<?> type) throws Exception {
    try {
      Field instance = type.getField("INSTANCE");
      if (Modifier.isStatic(instance.getModifiers())) {
        return instance.get(null);
      }
    } catch (NoSuchFieldException ignored) {
    }
    Constructor<?> constructor = type.getDeclaredConstructor();
    constructor.setAccessible(true);
    return constructor.newInstance();
  }

  private static String stackTrace(Throwable error) {
    StringWriter buffer = new StringWriter();
    error.printStackTrace(new PrintWriter(buffer));
    return buffer.toString();
  }

  private static void createParent(Path path) throws Exception {
    Path parent = path.getParent();
    if (parent != null) {
      Files.createDirectories(parent);
    }
  }

  private static String jsonString(String value) {
    StringBuilder out = new StringBuilder();
    out.append((char)34);
    for (int i = 0; i < value.length(); i++) {
      char ch = value.charAt(i);
      switch (ch) {
        case 34:
          out.append((char)92).append((char)34);
          break;
        case 92:
          out.append((char)92).append((char)92);
          break;
        case 10:
          out.append((char)92).append('n');
          break;
        case 13:
          out.append((char)92).append('r');
          break;
        case 9:
          out.append((char)92).append('t');
          break;
        default:
          if (ch < 32) {
            out.append((char)92).append('u').append(String.format("%04x", (int)ch));
          } else {
            out.append(ch);
          }
      }
    }
    out.append((char)34);
    return out.toString();
  }

  private static void field(StringBuilder out, String name) {
    out.append((char)34).append(name).append((char)34).append(':');
  }

  private static String reportJson(String target, String runnerType, List<TestCase> cases, int passed, int failed, String log, String nativeResults) {
    StringBuilder out = new StringBuilder();
    out.append('{');
    field(out, "schema");
    out.append(jsonString("once.test_results.v1")).append(',');
    field(out, "target");
    out.append(jsonString(target)).append(',');
    field(out, "runner");
    out.append('{');
    field(out, "type");
    out.append(jsonString(runnerType)).append(',');
    field(out, "metadata");
    out.append("{}},");
    field(out, "status");
    out.append(jsonString(failed == 0 ? "passed" : "failed")).append(',');
    field(out, "summary");
    out.append('{');
    field(out, "total");
    out.append(cases.size()).append(',');
    field(out, "passed");
    out.append(passed).append(',');
    field(out, "failed");
    out.append(failed).append(',');
    field(out, "skipped");
    out.append(0).append(',');
    field(out, "flaky");
    out.append(0).append("},");
    field(out, "cases");
    out.append('[');
    for (int i = 0; i < cases.size(); i++) {
      if (i > 0) {
        out.append(',');
      }
      TestCase testCase = cases.get(i);
      out.append('{');
      field(out, "id");
      out.append(jsonString(testCase.id)).append(',');
      field(out, "name");
      out.append(jsonString(testCase.name)).append(',');
      field(out, "suite");
      out.append(jsonString(testCase.suite)).append(',');
      field(out, "status");
      out.append(jsonString(testCase.status)).append(',');
      field(out, "attempts");
      out.append("[{");
      field(out, "status");
      out.append(jsonString(testCase.status)).append("}],");
      field(out, "runner_metadata");
      out.append("{}");
      out.append('}');
    }
    out.append("],");
    field(out, "artifacts");
    out.append('{');
    field(out, "logs");
    out.append('[').append(jsonString(log)).append("],");
    field(out, "native_results");
    out.append('[').append(jsonString(nativeResults)).append("]}}");
    out.append(System.lineSeparator());
    return out.toString();
  }
}
"""
