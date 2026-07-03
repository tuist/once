from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import subprocess


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        if self.path != "/toolbox/sandbox-1/process/execute":
            self.send_error(404)
            return
        if self.headers.get("Authorization") != "Bearer token-1":
            self.send_error(401)
            return
        length = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(length))
        proc = subprocess.run(
            request["command"],
            shell=True,
            cwd=request["cwd"],
            capture_output=True,
            text=True,
        )
        body = json.dumps(
            {
                "exitCode": proc.returncode,
                "artifacts": {
                    "stdout": proc.stdout,
                    "stderr": "daytona stderr\n" + proc.stderr,
                },
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


server = HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.handle_request()
