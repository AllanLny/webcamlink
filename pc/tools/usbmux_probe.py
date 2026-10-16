#!/usr/bin/env python3
"""WebcamLink usbmux probe (Windows).

Modes:
    list                          list devices visible through usbmuxd (default)
    connect <port>                open the tunnel to a device port (byte-swapped)
    echo <port> --send TEXT       connect, send raw bytes, print what comes back
    wcl-hello <port>              speak WCLN: send Hello, await Hello, heartbeat echo

usbmuxd on Windows (Apple Devices) listens on TCP 127.0.0.1:27015 and speaks the
libimobiledevice plist protocol: a 16-byte little-endian header
(length, version, type, tag) followed by an XML plist payload.

The Connect port must be byte-swapped: 5321 (0x14C9) -> 0xC914 = 51476.
"""

from __future__ import annotations

import argparse
import plistlib
import socket
import struct
import sys
import time

HEADER = struct.Struct("<IIII")
MSG_PLIST = 8
MAX_PAYLOAD = 1 << 20

USBMUX_HOST = "127.0.0.1"
USBMUX_PORT = 27015

WCL_MAGIC = b"WCLN"
WCL_HEADER = struct.Struct("<4sBBHI")  # magic, version, type, flags, length
WCL_VERSION = 1

MSG_HELLO = 0x01
MSG_HEARTBEAT = 0x30


def swapped_port(port: int) -> int:
    return ((port & 0xFF) << 8) | ((port >> 8) & 0xFF)


def mux_send(sock: socket.socket, tag: int, payload: dict) -> None:
    body = plistlib.dumps(payload, fmt=plistlib.FMT_XML)
    sock.sendall(HEADER.pack(len(body) + HEADER.size, 1, MSG_PLIST, tag) + body)


def read_exact(sock: socket.socket, size: int) -> bytes:
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise ConnectionError("connection closed")
        data += chunk
    return data


def mux_recv(sock: socket.socket) -> tuple[int, int, bytes]:
    header = read_exact(sock, HEADER.size)
    length, _version, message_type, tag = HEADER.unpack(header)
    payload = read_exact(sock, length - HEADER.size) if length > HEADER.size else b""
    return message_type, tag, payload


def connect_usbmux() -> socket.socket:
    sock = socket.create_connection((USBMUX_HOST, USBMUX_PORT), timeout=5)
    sock.settimeout(10)
    return sock


def list_devices(sock: socket.socket) -> list[dict]:
    mux_send(sock, 1, {"MessageType": "ListDevices", "ProgName": "webcamlink-probe", "ClientVersionString": "0.1"})
    _type, _tag, payload = mux_recv(sock)
    parsed = plistlib.loads(payload)
    return parsed.get("DeviceList", [])


def print_devices(devices: list[dict]) -> None:
    if not devices:
        print("0 appareil visible (iPhone branché, déverrouillé et approuvé ?)")
        return
    for device in devices:
        props = device.get("Properties", {})
        print(
            f"- DeviceID={device.get('DeviceID')} type={device.get('ConnectionType')} "
            f"name={props.get('DeviceName', '?')} udid={str(props.get('UniqueDeviceID', '?'))[:16]}…"
        )


def connect_device(sock: socket.socket, port: int) -> socket.socket:
    devices = list_devices(sock)
    usb = [d for d in devices if d.get("ConnectionType") == "USB"] or devices
    if not usb:
        raise SystemExit("Aucun appareil à connecter.")
    device_id = usb[0].get("DeviceID")
    tunnel = connect_usbmux()
    wire_port = swapped_port(port)
    print(f"Connect DeviceID={device_id} PortNumber={wire_port} (port {port} byte-swapped)")
    mux_send(
        tunnel,
        2,
        {
            "MessageType": "Connect",
            "DeviceID": device_id,
            "PortNumber": wire_port,
            "ProgName": "webcamlink-probe",
            "ClientVersionString": "0.1",
        },
    )
    _type, _tag, payload = mux_recv(tunnel)
    result = plistlib.loads(payload)
    number = result.get("Number", -1)
    meanings = {0: "OK", 2: "device invalide", 3: "connexion refusée (app non lancée ?)"}
    print(f"Résultat: {number} ({meanings.get(number, 'inconnu')})")
    if number != 0:
        tunnel.close()
        raise SystemExit(1)
    return tunnel


def wcl_encode(msg_type: int, payload: bytes, flags: int = 0) -> bytes:
    return WCL_HEADER.pack(WCL_MAGIC, WCL_VERSION, msg_type, flags, len(payload)) + payload


def wcl_writer_str(out: bytearray, value: str) -> None:
    raw = value.encode("utf-8")[:65535]
    out += struct.pack("<H", len(raw)) + raw


def wcl_hello_payload(name: str, model: str) -> bytes:
    body = bytearray()
    body += struct.pack("<BBB", 1, 1, 0)  # min, max, role=host
    wcl_writer_str(body, "0.1.0")
    wcl_writer_str(body, name)
    wcl_writer_str(body, model)
    return bytes(body)


def wcl_read_frame(sock: socket.socket, timeout: float = 5.0) -> tuple[int, bytes]:
    sock.settimeout(timeout)
    header = read_exact(sock, WCL_HEADER.size)
    magic, version, msg_type, _flags, length = WCL_HEADER.unpack(header)
    if magic != WCL_MAGIC:
        raise SystemExit(f"Trame non WCLN reçue: {magic!r}")
    if version != WCL_VERSION:
        raise SystemExit(f"Version de protocole inattendue: {version}")
    payload = read_exact(sock, length) if length else b""
    return msg_type, payload


def wcl_decode_hello(payload: bytes) -> dict:
    offset = 0

    def take(n: int) -> bytes:
        nonlocal offset
        data = payload[offset : offset + n]
        offset += n
        return data

    def take_str() -> str:
        nonlocal offset
        (length,) = struct.unpack("<H", take(2))
        return take(length).decode("utf-8", "replace")

    min_version, max_version, role = struct.unpack("<BBB", take(3))
    return {
        "min_version": min_version,
        "max_version": max_version,
        "role": role,
        "app_version": take_str(),
        "name": take_str(),
        "model": take_str(),
    }


def run_wcl_hello(tunnel: socket.socket) -> int:
    print("Envoi Hello…")
    tunnel.sendall(wcl_encode(MSG_HELLO, wcl_hello_payload(socket.gethostname(), "Windows-probe")))
    deadline = time.time() + 8
    while time.time() < deadline:
        msg_type, payload = wcl_read_frame(tunnel, timeout=8)
        if msg_type == MSG_HELLO:
            hello = wcl_decode_hello(payload)
            print(f"Hello reçu de l'iPhone: {hello}")
            break
        print(f"Message {msg_type:#x} reçu, ignoré")
    else:
        print("Pas de Hello reçu.")
        return 1

    stamp = time.monotonic_ns() // 1000
    tunnel.sendall(wcl_encode(MSG_HEARTBEAT, struct.pack("<Q", stamp)))
    msg_type, payload = wcl_read_frame(tunnel, timeout=5)
    if msg_type != MSG_HEARTBEAT:
        print(f"Réponse inattendue au heartbeat: {msg_type:#x}")
        return 1
    (echoed,) = struct.unpack("<Q", payload[:8])
    print(f"Heartbeat écho reçu (drift {abs(echoed - stamp)} µs)")
    print("TUNNEL USB OK — round-trip complet validé.")
    return 0


def run_lockdown(tunnel: socket.socket) -> int:
    """Round-trip through Apple's lockdownd (always listening): validates the
    whole chain — plist framing, byte-swapped Connect, device-side listener."""
    request = plistlib.dumps({"Label": "webcamlink-probe", "Request": "GetValue", "Key": "ProductVersion"})
    tunnel.sendall(request)
    data = tunnel.recv(65536)
    if not data:
        print("Pas de réponse de lockdownd.")
        return 1
    reply = plistlib.loads(data)
    print(f"lockdownd répond: ProductVersion={reply.get('Value')!r} (Type={reply.get('Type')!r})")
    print("CHAÎNE USB OK — plist + Connect byte-swapped + listener device validés.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", nargs="?", default="list", choices=["list", "connect", "echo", "lockdown", "wcl-hello"])
    parser.add_argument("port", nargs="?", type=int, default=5321)
    parser.add_argument("--send", default=None, help="texte brut à envoyer en mode echo")
    args = parser.parse_args()

    sock = connect_usbmux()

    if args.mode == "list":
        print_devices(list_devices(sock))
        return 0

    if args.mode == "connect":
        tunnel = connect_device(sock, args.port)
        print("Tunnel ouvert. (Ctrl+C pour fermer)")
        try:
            while True:
                data = tunnel.recv(4096)
                if not data:
                    print("Tunnel fermé par le device.")
                    break
                print(f"<- {len(data)} octets")
        except KeyboardInterrupt:
            pass
        return 0

    if args.mode == "echo":
        tunnel = connect_device(sock, args.port)
        if args.send:
            tunnel.sendall(args.send.encode("utf-8"))
            try:
                data = tunnel.recv(4096)
                print(f"Écho: {data!r}")
            except socket.timeout:
                print("Pas de réponse (timeout).")
        return 0

    if args.mode == "wcl-hello":
        tunnel = connect_device(sock, args.port)
        return run_wcl_hello(tunnel)

    if args.mode == "lockdown":
        tunnel = connect_device(sock, 62078)
        return run_lockdown(tunnel)

    return 2


if __name__ == "__main__":
    raise SystemExit(main())
