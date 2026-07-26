import { createServer } from "node:http";

const server = createServer((request, response) => {
  const url = new URL(request.url, "http://127.0.0.1");
  if (url.pathname === "/api/cache/endpoints") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ endpoints: ["grpc://127.0.0.1:19092"] }));
    return;
  }
  response.writeHead(404);
  response.end();
});

server.listen(18081, "127.0.0.1");
