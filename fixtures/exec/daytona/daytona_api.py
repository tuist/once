from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse
import json
import pathlib
import sys


deleted_marker = pathlib.Path(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def read_body(self):
        length = int(self.headers.get("Content-Length", "0"))
        return self.rfile.read(length)

    def respond(self, status, value=None):
        body = b"" if value is None else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self):
        if self.headers.get("Authorization") == "Bearer token-1":
            return True
        self.respond(401)
        return False

    def do_POST(self):
        if not self.authorized():
            return
        path = urlparse(self.path).path
        if path == "/api/sandbox":
            request = json.loads(self.read_body())
            if request.get("autoDeleteInterval") != 0:
                self.respond(400)
                return
            self.respond(
                200,
                {"id": "sandbox-1", "state": "started", "toolboxProxyUrl": None},
            )
            return
        if path == "/toolbox/sandbox-1/files/upload":
            self.read_body()
            self.respond(200)
            return
        if path == "/toolbox/sandbox-1/process/execute":
            request = json.loads(self.read_body())
            if request["command"].startswith("mkdir "):
                self.respond(200, {"exitCode": 0, "result": ""})
            else:
                value = request.get("envs", {}).get("VALUE", "missing")
                self.respond(200, {"exitCode": 0, "result": f"daytona-{value}"})
            return
        self.respond(404)

    def do_DELETE(self):
        if not self.authorized():
            return
        if urlparse(self.path).path != "/api/sandbox/sandbox-1":
            self.respond(404)
            return
        deleted_marker.write_text("deleted")
        self.respond(204)


server = HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
for _ in range(5):
    server.handle_request()
