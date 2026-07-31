import ELK from "elkjs/lib/elk.bundled.js"

const elk = new ELK()

export async function setupPassportGraphs(root = document) {
  for (const element of root.querySelectorAll("[data-passport-graph]")) {
    const graph = JSON.parse(element.dataset.passportGraph)
    const layout = await elk.layout({
      id: "graph",
      layoutOptions: {"elk.algorithm": "layered", "elk.direction": "RIGHT", "elk.spacing.nodeNode": "28", "elk.layered.spacing.nodeNodeBetweenLayers": "56"},
      children: graph.nodes.map((node) => ({...node, width: 196, height: 76})),
      edges: graph.edges.map((edge, index) => ({id: `edge-${index}`, sources: [edge.source], targets: [edge.target]}))
    })

    const svg = element.querySelector("svg")
    svg.setAttribute("viewBox", `0 0 ${layout.width + 24} ${layout.height + 24}`)
    svg.innerHTML = `<defs><marker id="passport-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto"><path d="M 0 0 L 8 4 L 0 8 z" /></marker></defs>${layout.edges.map(edge => `<path class="passport-graph-edge" marker-end="url(#passport-arrow)" d="${edge.sections.map(section => `M ${section.startPoint.x + 12} ${section.startPoint.y + 12} ${section.bendPoints.map(point => `L ${point.x + 12} ${point.y + 12}`).join(" ")} L ${section.endPoint.x + 12} ${section.endPoint.y + 12}`).join(" ")}" />`).join("")}${layout.children.map(node => `<g class="passport-graph-node" tabindex="0" data-node="${node.id}" transform="translate(${node.x + 12} ${node.y + 12})"><rect width="${node.width}" height="${node.height}" rx="8"/><text class="passport-graph-node-title" x="14" y="27">${escapeHtml(node.title)}</text><text class="passport-graph-node-detail" x="14" y="50">${node.cache === "hit" ? "Cache hit" : "Executed"} · ${node.duration_ms / 1000}s</text></g>`).join("")}`
    element.addEventListener("click", event => inspectNode(event.target.closest("[data-node]")?.dataset.node, graph, element))
  }
}

function inspectNode(id, graph, element) {
  const node = graph.nodes.find(node => node.id === id)
  if (!node) return
  element.querySelector("[data-part='graph-inspector']").innerHTML = `<strong>${escapeHtml(node.title)}</strong><span>${escapeHtml(node.command)}</span><span>${node.executor} executor · ${node.memory_mib} MiB memory · ${node.cache === "hit" ? "cache hit" : "executed"}</span>`
}

function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, character => ({"&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", "\"": "&quot;"}[character]))
}
