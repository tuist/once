import http.server
from pathlib import Path
import runpy
import socketserver
import sys


def bind_without_reverse_dns(server):
    socketserver.TCPServer.server_bind(server)
    server.server_name, server.server_port = server.server_address[:2]


# Numeric loopback fixture addresses must not depend on an external DNS lookup.
http.server.HTTPServer.server_bind = bind_without_reverse_dns
sys.argv = sys.argv[1:]
sys.path[0] = str(Path(sys.argv[0]).resolve().parent)
runpy.run_path(sys.argv[0], run_name="__main__")
