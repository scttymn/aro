#!/usr/bin/env python3
"""dexspec: read wire-format facts out of a `dexdump -d` of framework.jar.

The image is the ground truth for what an app's framework sends and expects.

  dexspec.py DUMP method  <class> <method>      # Parcel call sequence of one method
  dexspec.py DUMP parcel  <class>               # writeToParcel + constructor(Parcel) sequences
  dexspec.py DUMP methods <class>               # list methods of a class

Class names use dots (android.content.pm.ApplicationInfo). Output lines:
  PARCEL <op> <signature> [<- field or const]   for Parcel calls
  CALL   <target>                               for other calls (nested writers)
  IF/GOTO markers keep branches visible.
"""
import re
import sys

INVOKE = re.compile(r"\|[0-9a-f]{4}: invoke-(\w+)(?:/range)? \{([^}]*)\}, L([^;]+);\.([^:]+):(\S+)")
IGET = re.compile(r"\|[0-9a-f]{4}: [is]get(?:-\w+)? v(\d+), (?:v\d+, )?L([^;]+);\.([^:]+):(\S+)")
CONST = re.compile(r"\|[0-9a-f]{4}: const(?:/\w+)? v(\d+), #(?:int|float|long|double)? ?(-?\d+)")
BRANCH = re.compile(r"\|([0-9a-f]{4}): (if-\w+|goto(?:/\d+)?|packed-switch|sparse-switch|return\S*|throw)")
MOVERES = re.compile(r"\|[0-9a-f]{4}: move-result(?:-\w+)? v(\d+)")


def iter_methods(dump, cls):
    """Yield (method_name, signature, [lines]) for every method of `cls` (slash form)."""
    cur = None
    in_cls = False
    name = sig = None
    lines = []
    for line in dump:
        m = re.match(r"\s*Class descriptor\s*:\s*'L([^;]+);'", line)
        if m:
            if in_cls and name:
                yield name, sig, lines
            in_cls = m.group(1) == cls
            name = None
            lines = []
            continue
        if not in_cls:
            continue
        m = re.match(r"\s*#\d+\s*:\s*\(in L[^)]+\)", line)
        if m:
            if name:
                yield name, sig, lines
            name = sig = None
            lines = []
            continue
        m = re.match(r"\s*name\s*:\s*'([^']+)'", line)
        if m and name is None:
            name = m.group(1)
            continue
        m = re.match(r"\s*type\s*:\s*'([^']+)'", line)
        if m and sig is None:
            sig = m.group(1)
            continue
        if name:
            lines.append(line)
    if in_cls and name:
        yield name, sig, lines


def explain(lines):
    """Turn bytecode lines into a readable Parcel op sequence."""
    regs = {}  # register -> last known source (field name or const)
    out = []
    for line in lines:
        m = IGET.search(line)
        if m:
            regs[m.group(1)] = f"{m.group(3)}:{short(m.group(4))}"
            continue
        m = CONST.search(line)
        if m:
            regs[m.group(1)] = f"#{m.group(2)}"
            continue
        m = MOVERES.search(line)
        if m:
            regs[m.group(1)] = "(result)"
            continue
        m = BRANCH.search(line)
        if m:
            out.append(f"  {m.group(2)} @{m.group(1)}")
            continue
        m = INVOKE.search(line)
        if m:
            kind, args, cls, meth, sig = m.groups()
            argv = [a.strip() for a in args.split(",") if a.strip()]
            if cls == "android/os/Parcel":
                src = ", ".join(regs.get(a[1:], a) for a in argv[1:]) if len(argv) > 1 else ""
                out.append(f"PARCEL {meth} {sig}  <- {src}")
            else:
                out.append(f"CALL {cls.replace('/', '.')}.{meth}{sig}")
            continue
    return out


def short(sig):
    return sig.split("/")[-1].rstrip(";")


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    dump_path, cmd, cls = sys.argv[1], sys.argv[2], sys.argv[3].replace(".", "/")
    dump = open(dump_path, errors="replace")
    if cmd == "methods":
        for name, sig, _ in iter_methods(dump, cls):
            print(name, sig)
    elif cmd == "method":
        want = sys.argv[4]
        for name, sig, lines in iter_methods(dump, cls):
            if name == want:
                print(f"== {cls}.{name}{sig}")
                print("\n".join(explain(lines)))
    elif cmd == "codes":
        # For a $Stub$Proxy class: method name -> transaction code, from the
        # constant loaded before IBinder.transact (needed when the Stub keeps
        # no TRANSACTION_* static values).
        for name, sig, lines in iter_methods(dump, cls):
            last_const = {}
            for line in lines:
                m = re.search(r"\|[0-9a-f]{4}: const(?:/4|/16|/high16)? v(\d+), #int (-?\d+)", line)
                if m:
                    last_const[m.group(1)] = int(m.group(2))
                    continue
                m = re.search(r"invoke-interface \{v\d+, v(\d+), v\d+, v\d+, v\d+\}, Landroid/os/IBinder;\.transact", line)
                if m and m.group(1) in last_const:
                    print(f"{last_const[m.group(1)]} {name}")
                    break
    elif cmd == "parcel":
        for name, sig, lines in iter_methods(dump, cls):
            if name == "writeToParcel" or (name == "<init>" and "Landroid/os/Parcel;" in (sig or "")) or name == "readFromParcel":
                print(f"== {cls}.{name}{sig}")
                print("\n".join(explain(lines)))
                print()


if __name__ == "__main__":
    main()
