const cytoscapeModule = Promise.all([
  import("https://esm.sh/cytoscape@3.34.1?bundle"),
  import("https://esm.sh/cytoscape-dagre@2.5.0?bundle"),
])
  .then(([{default: cytoscape}, {default: dagre}]) => {
    cytoscape.use(dagre)
    return cytoscape
  })
  .catch((error) => {
    console.error("Could not load the Once graph", error)
    return null
  })

class OnceWorkspaceGraph extends HTMLElement {
  connectedCallback() {
    this.resizeObserver = new ResizeObserver(() => {
      this.graph?.resize()
    })
    this.resizeObserver.observe(this)
    this.renderGraph()
  }

  disconnectedCallback() {
    this.resizeObserver?.disconnect()
    this.graph?.destroy()
    this.graph = undefined
  }

  set graphData(graphData) {
    this.data = graphData
    if (this.isConnected) this.renderGraph()
  }

  async renderGraph() {
    const graphData = this.data
    if (!graphData?.nodes?.length) return
    const revision = (this.revision || 0) + 1
    this.revision = revision
    const cytoscape = await cytoscapeModule
    if (!this.isConnected || revision !== this.revision) return

    this.graph?.destroy()
    this.graph = undefined
    if (!cytoscape) {
      this.renderFallback(graphData)
      return
    }

    const controls = document.createElement("div")
    controls.dataset.part = "graph-controls"
    const fit = this.control("Fit graph")
    const reset = this.control("Reset view")
    controls.append(fit, reset)

    const canvas = document.createElement("div")
    canvas.dataset.part = "cytoscape-graph"
    this.replaceChildren(canvas, controls)

    try {
      this.graph = cytoscape({
        container: canvas,
        elements: this.elements(graphData),
        minZoom: 0.25,
        maxZoom: 2.5,
        wheelSensitivity: 0.16,
        style: [
          {
            selector: "node",
            style: {
              "background-color": "#ffffff",
              shape: "round-rectangle",
              "border-color": "#b9bec6",
              "border-width": 1,
              width: 180,
              height: 58,
              label: "data(label)",
              color: "#272a2f",
              "font-family": "Inter, ui-sans-serif, system-ui, sans-serif",
              "font-size": 13,
              "font-weight": 500,
              "text-valign": "center",
              "text-halign": "center",
              "text-wrap": "wrap",
              "text-max-width": 164,
              "overlay-opacity": 0,
            },
          },
          {
            selector: "node.aggregate",
            style: {
              "border-style": "dashed",
              color: "#5a5e65",
            },
          },
          {
            selector: "node.build-target",
            style: {
              "border-color": "#202124",
              "border-width": 2,
              width: 260,
              height: 92,
              "font-size": 15,
              "font-weight": 600,
              "text-max-width": 220,
            },
          },
          {
            selector: "node:selected",
            style: {
              "border-width": 3,
            },
          },
          {
            selector: "edge",
            style: {
              width: 1.15,
              "line-color": "#aeb4bc",
              "target-arrow-color": "#aeb4bc",
              "target-arrow-shape": "triangle",
              "curve-style": "bezier",
              "arrow-scale": 0.8,
              "overlay-opacity": 0,
            },
          },
        ],
        layout: {
          name: "dagre",
          rankDir: "LR",
          rankSep: 96,
          nodeSep: 38,
          padding: 54,
          animate: false,
          fit: true,
        },
      })
    } catch (error) {
      console.error("Could not draw the Once graph", error)
      this.renderFallback(graphData)
      return
    }

    fit.addEventListener("click", () => this.fitGraph())
    reset.addEventListener("click", () => {
      this.graph?.reset()
      this.fitGraph()
    })
    requestAnimationFrame(() => {
      this.graph?.resize()
      this.fitGraph()
    })
  }

  fitGraph() {
    if (!this.graph) return
    this.graph.fit(undefined, 54)
  }

  elements(graphData) {
    const nodeIds = new Set(graphData.nodes.map((node) => node.id))
    const elements = graphData.nodes.map((node) => ({
      group: "nodes",
      data: {id: node.id, label: this.label(node)},
      classes: [
        node.grouped_dependency_count > 0 && "aggregate",
        node.build_target && "build-target",
      ].filter(Boolean).join(" "),
    }))
    for (const node of graphData.nodes) {
      for (const dependency of node.deps) {
        if (!nodeIds.has(dependency)) continue
        elements.push({
          group: "edges",
          data: {id: `${dependency}->${node.id}`, source: dependency, target: node.id},
        })
      }
    }
    return elements
  }

  label(node) {
    const name = node.name === "cargo_dependencies"
      ? "External packages"
      : node.name.replaceAll("_", "-")
    if (node.build_target) return `${name}\nBuild target\n${node.package || node.id}`
    if (node.grouped_dependency_count > 0) {
      return `${name}\n${node.grouped_dependency_count} resolved packages`
    }
    return name
  }

  renderFallback(graphData) {
    const fallback = document.createElement("div")
    fallback.dataset.part = "graph-fallback"
    const message = document.createElement("p")
    message.textContent = "The interactive graph could not be loaded."
    const targets = document.createElement("ul")
    for (const node of graphData.nodes) {
      const target = document.createElement("li")
      target.textContent = this.label(node).replaceAll("\n", " · ")
      targets.append(target)
    }
    fallback.append(message, targets)
    this.replaceChildren(fallback)
  }

  control(label) {
    const button = document.createElement("noora-button")
    button.setAttribute("size", "small")
    button.setAttribute("variant", "secondary")
    button.textContent = label
    return button
  }
}

customElements.define("once-workspace-graph", OnceWorkspaceGraph)

const app = document.querySelector("#app")
const staticRun = globalThis.__ONCE_RUN__ || null
let run = staticRun
const testView = {
  query: "",
  status: "all",
  sort: "name",
  direction: "asc",
}

function escape(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;")
}

function route() {
  const page = staticRun
    ? window.location.hash.slice(1) || "overview"
    : window.location.pathname.split("/").filter(Boolean).at(-1) || "overview"
  if (page === "tests" && run?.operation !== "test") return "overview"
  return ["overview", "progress", "tests"].includes(page) ? page : "overview"
}

function statusLabel(status) {
  if (status === "completed") return "Completed"
  if (status === "failed") return "Failed"
  return "Running"
}

function badgeColor(status) {
  if (status === "completed") return "success"
  if (status === "failed") return "destructive"
  return "primary"
}

function cacheLabel(snapshot) {
  if (snapshot.cache === "hit") return "Hit"
  if (snapshot.cache === "miss") return "Miss"
  return snapshot.status === "failed" ? "Not reached" : "Pending"
}

function cacheDetail(snapshot) {
  if (snapshot.cache === "hit") return "Restored from the action cache"
  if (snapshot.cache === "miss") return "Executed locally"
  return snapshot.status === "failed"
    ? "Build stopped before the cache decision"
    : "Awaiting a cache decision"
}

function duration(snapshot) {
  return snapshot.duration_ms == null ? "Running" : `${snapshot.duration_ms} ms`
}

function exitLabel(snapshot) {
  return snapshot.exit_code == null ? "Awaiting completion" : `Exit code ${snapshot.exit_code}`
}

function buildLabel(snapshot) {
  return (snapshot.target || snapshot.action_digest || "run").split("/").filter(Boolean).at(-1)
}

function operationLabel(snapshot) {
  return snapshot.operation === "test" ? "Test" : "Build"
}

function time(value) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(new Date(value))
}

function routePath(page) {
  return staticRun ? `#${page}` : `/runs/${page}`
}

function staticReportUrl(path) {
  const encodedPath = path.split("/").map(encodeURIComponent).join("/")
  return `file://${encodedPath}`
}

function navItem(page, label, icon, selected) {
  return `<noora-sidebar-item href="${routePath(page)}" data-route="${page}" icon="${icon}"${selected ? " selected" : ""}>${label}</noora-sidebar-item>`
}

function shell(content) {
  const crumb = run
    ? `<noora-breadcrumb-item>${escape(run.workspace)}</noora-breadcrumb-item>
       <noora-breadcrumb-item>${escape(run.target)}</noora-breadcrumb-item>`
    : ""
  return `<div data-part="runs-shell">
    <header data-part="runs-header">
      <a data-part="runs-brand" href="${routePath("overview")}" data-route="overview" aria-label="Once Runs">
        <noora-icon name="package"></noora-icon><span>Once</span>
      </a>
      <noora-breadcrumbs data-part="runs-breadcrumbs" style-variant="slash">
        <noora-breadcrumb-item href="${routePath("overview")}" icon="stack_2" data-route="overview">Runs</noora-breadcrumb-item>
        ${crumb}
      </noora-breadcrumbs>
      <div data-part="runs-header-actions">
        <noora-button href="https://github.com/tuist/once" target="_blank" variant="secondary" size="medium">
          <noora-icon slot="icon-left" name="book"></noora-icon>Docs
        </noora-button>
      </div>
    </header>
    <section data-part="runs-layout">
      <aside data-part="runs-sidebar">
        <noora-sidebar label="Run navigation">
          ${navItem("overview", "Overview", "dashboard", route() === "overview")}
          ${navItem("progress", "Progress", "progress_x", route() === "progress")}
          ${run?.operation === "test" ? navItem("tests", "Test results", "progress_x", route() === "tests") : ""}
        </noora-sidebar>
      </aside>
      <main data-part="runs-content">${content}</main>
    </section>
  </div>`
}

function titlebar(snapshot) {
  return `<header data-part="run-titlebar">
    <div data-part="run-eyebrow">
      <span>${escape(snapshot.workspace)}</span><span data-part="run-eyebrow-separator">/</span><span>Once ${escape(snapshot.operation || "build")}</span>
    </div>
    <div data-part="run-heading">
      <h1>${operationLabel(snapshot)} ${escape(buildLabel(snapshot))}</h1>
      <noora-badge appearance="light-fill" color="${badgeColor(snapshot.status)}">${statusLabel(snapshot.status)}</noora-badge>
    </div>
    <p data-part="run-command"><code>${escape(snapshot.command)}</code></p>
  </header>`
}

function runMetric(title, description, value, detail) {
  return `<section class="noora-card__section tuist-widget" data-part="widget">
    <div data-part="header">
      <div data-part="title"><span data-part="label">${escape(title)}</span></div>
      <noora-tooltip size="large" title="${escape(title)}" description="${escape(description)}">
        <noora-icon slot="trigger" name="alert_circle"></noora-icon>
      </noora-tooltip>
    </div>
    <span data-part="value">${escape(value)}</span>
    <div data-part="trend">
      <noora-badge appearance="light-fill" color="neutral">Current</noora-badge>
      <span data-part="label">${escape(detail)}</span>
    </div>
  </section>`
}

function runMetrics(snapshot) {
  const resolvedTargetCount = snapshot.graph?.resolved_target_count
  const outputCount = Array.isArray(snapshot.logs) ? snapshot.logs.length : 0
  const metrics = [
    runMetric(
      "Cache decision",
      "Shows whether Once restored the target from the action cache or ran it locally.",
      cacheLabel(snapshot),
      cacheDetail(snapshot),
    ),
    runMetric(
      `${operationLabel(snapshot)} duration`,
      "Measures the complete Once run from startup through the final result.",
      duration(snapshot),
      snapshot.status === "running" ? "Measuring the current run" : "Recorded by Once",
    ),
    runMetric(
      "Resolved targets",
      "Counts the targets Once resolved while preparing this run.",
      resolvedTargetCount ?? "Loading",
      resolvedTargetCount == null
        ? "Waiting for the target graph"
        : `${resolvedTargetCount} targets in this run`,
    ),
  ]
  if (snapshot.operation === "test") {
    const testCount = numberValue(snapshot.test_results?.summary?.total)
    metrics.push(runMetric(
      "Test cases",
      "Counts the test cases reported by this test run.",
      testCount ?? "Pending",
      testCount == null ? "Waiting for test results" : "Reported by the test runner",
    ))
  } else {
    metrics.push(runMetric(
      "Output updates",
      "Counts the recent output updates retained for this run.",
      outputCount,
      snapshot.output_truncated ? "Showing the most recent output" : "Captured during this run",
    ))
  }
  return metrics.join("")
}

function overview(snapshot) {
  return `<section data-part="run-page">
    ${titlebar(snapshot)}
    <section data-part="widgets" aria-label="Run analytics">${runMetrics(snapshot)}</section>
    <section data-part="run-details">
      <noora-card icon="info_circle" title="Run details">
        <div data-part="build-metadata-grid">
          <div data-part="build-metadata" data-wide><span data-part="build-metadata-title">Command</span><code>${escape(snapshot.command)}</code></div>
          <div data-part="build-metadata"><span data-part="build-metadata-title">Workspace</span><code>${escape(snapshot.workspace)}</code></div>
          <div data-part="build-metadata"><span data-part="build-metadata-title">Target</span><code>${escape(snapshot.target)}</code></div>
          <div data-part="build-metadata"><span data-part="build-metadata-title">Started</span><span>${time(snapshot.started_at_ms)}</span></div>
          <div data-part="build-metadata"><span data-part="build-metadata-title">Action digest</span><code>${escape(snapshot.action_digest)}</code></div>
        </div>
      </noora-card>
    </section>
  </section>`
}

function logLabel(entry) {
  if (entry.stream === "stderr") return "Error output"
  if (entry.stream === "stdout") return "Output"
  return "Build update"
}

function outputEntries(snapshot) {
  const logs = snapshot.logs || []
  if (!logs.length) return `<p data-part="run-message">Waiting for the build to produce output.</p>`
  return logs.map((entry) => `<article data-part="run-log-entry">
    <div data-part="run-log-entry-header">
      <noora-badge appearance="light-fill" color="${entry.stream === "stderr" ? "destructive" : "secondary"}">${logLabel(entry)}</noora-badge>
      <time>${time(entry.at_ms)}</time>
    </div>
    <pre data-part="run-log-output"><code>${escape(entry.text)}</code></pre>
  </article>`).join("")
}

function progress(snapshot) {
  const operation = operationLabel(snapshot)
  const graph = snapshot.graph
  const graphBody = graph
    ? `<div data-part="graph-toolbar">
        <noora-badge appearance="light-fill" color="primary">${graph.declared_target_count} targets in this build</noora-badge>
        <span data-part="graph-source">${graph.resolved_target_count} resolved targets loaded by Once</span>
      </div>
      <div data-part="build-graph"><once-workspace-graph id="workspace-graph"></once-workspace-graph></div>`
    : `<p data-part="run-message">Waiting for Once to resolve the build graph.</p>`
  return `<section data-part="run-page">
    ${titlebar(snapshot)}
    <section data-part="run-workspace">
      <noora-card icon="schema" title="${operation} graph">${graphBody}</noora-card>
    </section>
    <section data-part="run-output" aria-label="Build activity">
      <noora-card icon="devices_code" title="${operation} output">
        <div data-part="run-output-toolbar">
          <noora-badge appearance="light-fill" color="${badgeColor(snapshot.status)}">${snapshot.status === "running" ? "Live" : "Recorded"}</noora-badge>
          <span data-part="run-output-limit">${snapshot.output_truncated ? "Showing the most recent output" : "Updates from the running build"}</span>
        </div>
        <div data-part="run-log-list" aria-live="polite">${outputEntries(snapshot)}</div>
      </noora-card>
    </section>
  </section>`
}

function numberValue(value) {
  const number = Number(value)
  return Number.isFinite(number) ? number : null
}

function caseDuration(testCase) {
  const direct = [
    testCase.duration_ms,
    testCase.duration,
    testCase.runner_metadata?.duration_ms,
    testCase.runner_metadata?.duration,
  ]
  for (const value of direct) {
    const duration = numberValue(value)
    if (duration != null) return duration
  }
  const attempts = Array.isArray(testCase.attempts) ? testCase.attempts : []
  const durations = attempts
    .map((attempt) => numberValue(attempt.duration_ms ?? attempt.duration))
    .filter((duration) => duration != null)
  return durations.length ? durations.reduce((total, duration) => total + duration, 0) : null
}

function durationLabel(duration) {
  if (duration == null) return "Not reported"
  if (duration < 1000) return `${Math.round(duration)} ms`
  if (duration < 60_000) return `${(duration / 1000).toFixed(duration % 1000 ? 1 : 0)} s`
  return `${Math.floor(duration / 60_000)}m ${Math.round((duration % 60_000) / 1000)}s`
}

function testStatus(value) {
  if (value === "success") return "passed"
  if (value === "failure") return "failed"
  return value || "unknown"
}

function isFlaky(testCase) {
  const attempts = Array.isArray(testCase.attempts) ? testCase.attempts : []
  return new Set(attempts.map((attempt) => testStatus(attempt.status))).size > 1 || testStatus(testCase.status) === "flaky"
}

function testStatusLabel(status) {
  if (status === "passed") return "Passed"
  if (status === "failed") return "Failed"
  if (status === "skipped") return "Skipped"
  if (status === "flaky") return "Flaky"
  return "Unknown"
}

function testCases(snapshot) {
  const cases = Array.isArray(snapshot.test_results?.cases) ? snapshot.test_results.cases : []
  const query = testView.query.trim().toLocaleLowerCase()
  const filtered = cases.filter((testCase) => {
    const status = testStatus(testCase.status)
    const matchesQuery = !query || [testCase.name, testCase.suite, testCase.id].some((value) => String(value || "").toLocaleLowerCase().includes(query))
    const matchesStatus = testView.status === "all" || (testView.status === "flaky" ? isFlaky(testCase) : status === testView.status)
    return matchesQuery && matchesStatus
  })
  return filtered.sort((left, right) => {
    let comparison
    if (testView.sort === "duration") comparison = (caseDuration(left) ?? -1) - (caseDuration(right) ?? -1)
    else if (testView.sort === "status") comparison = testStatus(left.status).localeCompare(testStatus(right.status))
    else comparison = String(left.name || left.id).localeCompare(String(right.name || right.id))
    return testView.direction === "asc" ? comparison : -comparison
  })
}

function testSortLabel() {
  if (testView.sort === "duration") return "Duration"
  if (testView.sort === "status") return "Status"
  return "Test case"
}

function averageTestDuration(snapshot) {
  const cases = Array.isArray(snapshot.test_results?.cases) ? snapshot.test_results.cases : []
  const durations = cases.map(caseDuration).filter((value) => value != null)
  if (!durations.length) return null
  return durations.reduce((total, value) => total + value, 0) / durations.length
}

function testWidget(title, description, value) {
  return `<section slot="section" data-part="test-widget">
    <div data-part="header">
      <div data-part="title"><span data-part="label">${title}</span></div>
      <noora-tooltip size="large" title="${title}" description="${description}">
        <noora-icon slot="trigger" name="alert_circle"></noora-icon>
      </noora-tooltip>
    </div>
    <span data-part="value">${value}</span>
  </section>`
}

function testSummary(snapshot) {
  const summary = snapshot.test_results?.summary || {}
  const total = numberValue(summary.total) ?? 0
  const failed = numberValue(summary.failed) ?? 0
  const flaky = numberValue(summary.flaky) ?? 0
  return `${testWidget("Test cases", "Total number of test cases executed.", total)}
    ${testWidget("Failed test cases", "Number of test cases that failed.", failed)}
    ${testWidget("Flaky test cases", "Number of test cases that passed after retry.", flaky)}
    ${testWidget("Avg. test case duration", "Average duration of all test cases.", durationLabel(averageTestDuration(snapshot)))}`
}

function testCaseRows(snapshot) {
  return testCases(snapshot).map((testCase) => {
    const status = testStatus(testCase.status)
    const attempts = Array.isArray(testCase.attempts) ? testCase.attempts : []
    const flaky = isFlaky(testCase)
    const name = testCase.name || testCase.id
    const suite = testCase.suite ? ` · ${testCase.suite}` : ""
    const result = `${testStatusLabel(status)}${flaky ? " · Flaky" : ""}`
    return `<noora-table-row row-key="${escape(testCase.id)}">
      <noora-table-cell column="name">${escape(name + suite)}</noora-table-cell>
      <noora-table-cell column="attempts">${attempts.length}</noora-table-cell>
      <noora-table-cell column="duration">${durationLabel(caseDuration(testCase))}</noora-table-cell>
      <noora-table-cell column="status">${escape(result)}</noora-table-cell>
    </noora-table-row>`
  }).join("")
}

function testEmptyState() {
  const filtered = testView.query.trim().length > 0 || testView.status !== "all"
  const subtitle = filtered
    ? "Try updating your search or filters."
    : "This test target did not report any test cases."
  return `<div slot="empty" data-part="test-table-empty-state">
    <noora-icon name="subtask"></noora-icon>
    <strong>No test cases found</strong>
    <span>${subtitle}</span>
  </div>`
}

function testResults(snapshot) {
  if (!snapshot.test_results) {
    return `<section data-part="run-page">
      ${titlebar(snapshot)}
      <noora-card icon="progress_x" title="Test results"><p data-part="run-message">Test results are not available yet. Once will add normalized test cases when the runner publishes them.</p></noora-card>
    </section>`
  }
  const cases = testCases(snapshot)
  const order = testView.direction === "asc" ? "asc" : "desc"
  return `<section data-part="run-page">
    ${titlebar(snapshot)}
    <noora-card data-part="test-analytics-card" icon="progress_x" title="Test results">
      ${testSummary(snapshot)}
    </noora-card>
    <noora-card data-part="test-results-card" icon="subtask" title="Test cases">
      <section slot="section" data-part="test-cases-section">
        <noora-tab-menu value="cases" data-part="test-results-tabs"><noora-tab-item value="cases">Test cases</noora-tab-item></noora-tab-menu>
        <div data-part="test-controls">
          <noora-text-input data-test-search type="search" name="test-search" placeholder="Search test cases" value="${escape(testView.query)}"></noora-text-input>
          <noora-dropdown data-test-sort label="${testSortLabel()}" secondary-text="Sort by:">
            <noora-dropdown-item value="name">Test case</noora-dropdown-item>
            <noora-dropdown-item value="duration">Duration</noora-dropdown-item>
            <noora-dropdown-item value="status">Status</noora-dropdown-item>
          </noora-dropdown>
          <noora-dropdown data-test-filter label="${testView.status === "all" ? "All results" : testStatusLabel(testView.status)}" secondary-text="Filter:">
            <noora-dropdown-item value="all">All results</noora-dropdown-item>
            <noora-dropdown-item value="passed">Passed</noora-dropdown-item>
            <noora-dropdown-item value="failed">Failed</noora-dropdown-item>
            <noora-dropdown-item value="skipped">Skipped</noora-dropdown-item>
            <noora-dropdown-item value="flaky">Flaky</noora-dropdown-item>
          </noora-dropdown>
        </div>
        ${testView.status !== "all" ? `<div data-part="test-active-filters"><noora-tag data-test-clear-filter label="${testStatusLabel(testView.status)}" dismissible></noora-tag></div>` : ""}
        <noora-table row-key="id">
          <noora-table-column name="name" sort-order="${testView.sort === "name" ? order : ""}">Test case</noora-table-column>
          <noora-table-column name="attempts">Attempts</noora-table-column>
          <noora-table-column name="duration" sort-order="${testView.sort === "duration" ? order : ""}">Duration</noora-table-column>
          <noora-table-column name="status" sort-order="${testView.sort === "status" ? order : ""}">Status</noora-table-column>
          ${cases.length ? testCaseRows(snapshot) : testEmptyState()}
        </noora-table>
      </section>
    </noora-card>
  </section>`
}

function empty() {
  return `<section data-part="run-empty">
    <noora-card icon="player_play" title="Waiting for a build">
      <p data-part="run-message">The local Runs page will attach as soon as Once starts this build.</p>
    </noora-card>
  </section>`
}

function render() {
  const currentRoute = route()
  const content = run ? (currentRoute === "progress" ? progress(run) : currentRoute === "tests" ? testResults(run) : overview(run)) : empty()
  app.innerHTML = shell(content)
  const graph = app.querySelector("#workspace-graph")
  if (graph && run?.graph) graph.graphData = run.graph
  bindTestControls()
}

function bindTestControls() {
  const search = app.querySelector("[data-test-search]")
  search?.addEventListener("input", (event) => {
    testView.query = event.target.value
    render()
  })
  app.querySelector("[data-test-sort]")?.addEventListener("noora-select", (event) => {
    const sort = String(event.detail.value)
    testView.direction = testView.sort === sort ? (testView.direction === "asc" ? "desc" : "asc") : sort === "name" ? "asc" : "desc"
    testView.sort = sort
    render()
  })
  app.querySelector("[data-test-filter]")?.addEventListener("noora-select", (event) => {
    testView.status = String(event.detail.value)
    render()
  })
  app.querySelector("[data-test-clear-filter]")?.addEventListener("noora-dismiss", () => {
    testView.status = "all"
    render()
  })
}

function applyProgressUpdate(previous) {
  if (route() !== "progress" || !previous || previous.run_id !== run.run_id) return false
  if (previous.status !== run.status || Boolean(previous.graph) !== Boolean(run.graph)) return false
  const output = app.querySelector('[data-part="run-log-list"]')
  if (!output) return false
  output.innerHTML = outputEntries(run)
  return true
}

function openStaticReport(previous) {
  if (staticRun || !run?.static_report_path || previous?.static_report_path === run.static_report_path) return false
  window.location.replace(staticReportUrl(run.static_report_path))
  return true
}

function navigate(event) {
  const link = event.target.closest("[data-route], noora-sidebar-item")
  const page = link?.getAttribute("data-route")
  if (page !== "overview" && page !== "progress" && page !== "tests") return
  if (page === "tests" && run?.operation !== "test") return
  event.preventDefault()
  const destination = routePath(page)
  const current = staticRun ? window.location.hash : window.location.pathname
  if (current !== destination) window.history.pushState({}, "", destination)
  render()
}

async function loadInitialState() {
  if (staticRun) {
    render()
    return
  }
  try {
    const response = await fetch("/api/runs/latest", {cache: "no-store"})
    if (response.ok) run = await response.json()
  } catch (error) {
    console.error("Could not load the current Once build", error)
  }
  if (openStaticReport(null)) return
  render()
}

function connect() {
  if (staticRun) return
  const events = new EventSource("/api/runs/events")
  events.addEventListener("state", (event) => {
    try {
      const previous = run
      run = JSON.parse(event.data)
      if (openStaticReport(previous)) return
      if (!applyProgressUpdate(previous)) render()
    } catch (error) {
      console.error("Could not apply a Once build update", error)
    }
  })
}

document.addEventListener("click", navigate)
window.addEventListener("popstate", render)
render()
void loadInitialState()
connect()
