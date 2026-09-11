#!/usr/bin/env python3
"""JUCE plugin-state containers for the shared plugin pack.

A muz `plugin`/`piano` device hands the file named by `state:` to the plugin
byte for byte. Three shapes occur, and this module converts between the
editable payload and the stored container:

* ``VC2!`` - ``b"VC2!" + u32le(payload length) + payload + b"\\0"``. OrbitCab's
  ``.state`` files use this container.
* ``VstW`` - the generic wrapper ``magic + u32le(payload length) + payload``.
  Pass ``magic=b"sub3"`` for Surge XT's variant, which reserves 32 header bytes
  and appends a trailing block after the payload.
* raw XML - the editable form, stored with no container at all.

Every function is a pure byte transform, so a stored state reproduces exactly
and `--check` can prove it.
"""

from __future__ import annotations

import argparse
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

VC2_MAGIC = b"VC2!"
VSTW_MAGIC = b"VstW"
SURGE_MAGIC = b"sub3"

VC2_HEADER = 8
VSTW_HEADER = 8
SURGE_HEADER = 32

__all__ = [
    "VC2_MAGIC",
    "VSTW_MAGIC",
    "SURGE_MAGIC",
    "State",
    "decode",
    "wrap_vc2_state",
    "unwrap_vc2_state",
    "wrap_vst3_state",
    "unwrap_vst3_state",
]


def _header_size(magic: bytes) -> int:
    return SURGE_HEADER if magic == SURGE_MAGIC else VSTW_HEADER


def wrap_vc2_state(payload: bytes, trailer: bytes = b"\x00") -> bytes:
    """Return the VC2! container for an XML payload."""
    return VC2_MAGIC + struct.pack("<I", len(payload)) + payload + trailer


def unwrap_vc2_state(state: bytes) -> tuple[bytes, bytes]:
    """Return ``(payload, trailer)`` from a VC2! container."""
    if not state.startswith(VC2_MAGIC):
        raise ValueError("not a VC2! state: expected a b'VC2!' prefix")
    size = struct.unpack_from("<I", state, 4)[0]
    end = VC2_HEADER + size
    if len(state) < end:
        raise ValueError(f"VC2! length field {size} exceeds the {len(state)}-byte state")
    return state[VC2_HEADER:end], state[end:]


def wrap_vst3_state(
    payload: bytes,
    magic: bytes = VSTW_MAGIC,
    header: bytes | None = None,
    trailer: bytes = b"",
) -> bytes:
    """Return a VST3-style container holding ``payload``.

    ``magic`` selects the header layout: ``VstW`` uses 8 header bytes, ``sub3``
    uses 32. Pass ``header`` to reuse a stored header's reserved bytes (its
    magic and length field are rewritten); otherwise a zeroed header is built.
    ``trailer`` is appended unchanged after the payload.
    """
    head = bytearray(_header_size(magic) if header is None else header)
    if len(head) < 8:
        raise ValueError(f"a {magic!r} header needs at least 8 bytes, got {len(head)}")
    head[:4] = magic
    struct.pack_into("<I", head, 4, len(payload))
    return bytes(head) + payload + trailer


def unwrap_vst3_state(state: bytes, magic: bytes = VSTW_MAGIC) -> tuple[bytes, bytes, bytes]:
    """Return ``(payload, header, trailer)`` from a VstW/sub3 container."""
    if not state.startswith(magic):
        raise ValueError(f"not a {magic!r} state: expected a {magic!r} prefix")
    header_size = _header_size(magic)
    size = struct.unpack_from("<I", state, 4)[0]
    end = header_size + size
    if len(state) < end:
        raise ValueError(
            f"{magic!r} length field {size} exceeds the {len(state)}-byte state"
        )
    return state[header_size:end], state[:header_size], state[end:]


@dataclass(frozen=True)
class State:
    """One plugin state: container form, editable payload, and byte envelope."""

    form: str
    payload: bytes
    header: bytes = b""
    trailer: bytes = b""

    def encode(self) -> bytes:
        """Rebuild the stored bytes; the inverse of :func:`decode`."""
        if self.form == "xml":
            return self.payload
        if self.form == "VC2":
            return wrap_vc2_state(self.payload, self.trailer)
        return wrap_vst3_state(
            self.payload, magic=self.header[:4], header=self.header, trailer=self.trailer
        )


def decode(state: bytes) -> State:
    """Split stored state bytes into their container form and editable payload."""
    if state.startswith(VC2_MAGIC):
        payload, trailer = unwrap_vc2_state(state)
        return State("VC2", payload, VC2_MAGIC + b"\x00" * 4, trailer)
    for magic in (VSTW_MAGIC, SURGE_MAGIC):
        if state.startswith(magic):
            payload, header, trailer = unwrap_vst3_state(state, magic)
            return State(magic.decode("ascii"), payload, header, trailer)
    if state.lstrip().startswith(b"<"):
        return State("xml", state)
    raise ValueError(
        "unrecognized plugin state: expected a VC2!/VstW/sub3 container or raw XML"
    )


def _check(state_path: Path, xml_path: Path | None, magic: str) -> int:
    state = state_path.read_bytes()
    form = decode(state)
    if magic != "auto" and form.form != magic:
        print(f"{state_path}: is {form.form}, not {magic}", file=sys.stderr)
        return 1
    rebuilt = form.encode()
    status = 0
    if rebuilt == state:
        print(
            f"{state_path}: {form.form} round-trip matches "
            f"({len(state)} bytes, {len(form.payload)} payload bytes)"
        )
    else:
        print(
            f"{state_path}: {form.form} round-trip DIFFERS "
            f"({len(state)} stored, {len(rebuilt)} rebuilt)",
            file=sys.stderr,
        )
        status = 1
    if xml_path is not None:
        payload = xml_path.read_bytes()
        if payload == form.payload:
            print(f"{xml_path}: payload matches the stored state")
        else:
            print(
                f"{xml_path}: payload DIFFERS from the stored state "
                f"({len(payload)} file bytes, {len(form.payload)} state payload bytes)",
                file=sys.stderr,
            )
            status = 1
    return status


def _main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="juce_state.py",
        description="Wrap and unwrap the JUCE plugin-state containers used by this pack.",
        epilog="examples:\n"
        "  juce_state.py --check preset.state --xml preset.xml\n"
        "  juce_state.py --wrap preset.xml --state preset.state\n"
        "  juce_state.py --wrap preset.xml --state preset.state --magic sub3 --template old.state\n"
        "  juce_state.py --unwrap preset.state --payload preset.xml",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--check", metavar="STATE", help="decode STATE, re-encode it, and report whether the bytes match")
    parser.add_argument("--wrap", metavar="PAYLOAD", help="wrap the payload file into a state container")
    parser.add_argument("--unwrap", metavar="STATE", help="write the editable payload held by a state container")
    parser.add_argument("--xml", metavar="XML", help="with --check, also compare this editable payload")
    parser.add_argument("--state", metavar="STATE", help="destination for --wrap")
    parser.add_argument("--payload", metavar="PAYLOAD", help="destination for --unwrap")
    parser.add_argument("--template", metavar="STATE", help="with --wrap, reuse this state's container, header and trailer")
    parser.add_argument(
        "--magic",
        choices=("auto", "VC2", "VstW", "sub3"),
        default="auto",
        help="container to write (--wrap) or require (--check/--unwrap); default auto",
    )
    args = parser.parse_args(argv)

    if args.check:
        return _check(Path(args.check), Path(args.xml) if args.xml else None, args.magic)

    if args.wrap:
        if not args.state:
            parser.error("--wrap needs --state")
        payload = Path(args.wrap).read_bytes()
        if args.template:
            form = decode(Path(args.template).read_bytes())
            if args.magic not in ("auto", form.form):
                parser.error(f"--template holds {form.form}, not --magic {args.magic}")
            out = State(form.form, payload, form.header, form.trailer).encode()
        else:
            if args.magic == "sub3":
                out = wrap_vst3_state(payload, magic=SURGE_MAGIC)
            elif args.magic == "VstW":
                out = wrap_vst3_state(payload, magic=VSTW_MAGIC)
            else:
                out = wrap_vc2_state(payload)
        Path(args.state).write_bytes(out)
        print(f"wrote {args.state} ({len(payload)} payload bytes, {len(out)} state bytes)")
        return 0

    if args.unwrap:
        if not args.payload:
            parser.error("--unwrap needs --payload")
        form = decode(Path(args.unwrap).read_bytes())
        if args.magic not in ("auto", form.form):
            parser.error(f"{args.unwrap} holds {form.form}, not --magic {args.magic}")
        Path(args.payload).write_bytes(form.payload)
        print(f"wrote {args.payload} ({len(form.payload)} payload bytes from {form.form})")
        return 0

    parser.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(_main())
