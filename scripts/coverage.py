#!/usr/bin/env python3
import sys, os, subprocess, json, glob, re

rustc_sysroot = subprocess.check_output(["rustc", "--print", "sysroot"]).decode().strip()
llvm_cov = os.path.join(rustc_sysroot, "lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-cov")
llvm_profdata = os.path.join(rustc_sysroot, "lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata")

def run_tests():
    # Clean up any existing profraw files
    for root, dirs, files in os.walk("."):
        for f in files:
            if f.endswith(".profraw"):
                os.remove(os.path.join(root, f))
    
    env = os.environ.copy()
    env["RUSTFLAGS"] = "-C instrument-coverage"
    env["LLVM_PROFILE_FILE"] = os.path.abspath("target/all_cov/tigrs-%p-%m.profraw")
    os.makedirs("target/all_cov", exist_ok=True)
    res = subprocess.run(["cargo", "test", "--workspace", "--tests"], env=env)
    if res.returncode != 0:
        print("Tests failed!")
        sys.exit(res.returncode)

def get_binaries():
    cmd = ["cargo", "test", "--workspace", "--tests", "--no-run", "--message-format=json"]
    env = os.environ.copy()
    env["RUSTFLAGS"] = "-C instrument-coverage"
    p = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    binaries = []
    for line in p.stdout:
        data = json.loads(line)
        if data.get("executable") and (
            data.get("profile", {}).get("test")
            or data.get("target", {}).get("kind") == ["bin"]
        ):
            if data["executable"] not in binaries:
                binaries.append(data["executable"])
    p.wait()
    return binaries

def merge_profdata():
    profraws = []
    for root, dirs, files in os.walk("."):
        for f in files:
            if f.endswith(".profraw"):
                profraws.append(os.path.join(root, f))
    os.makedirs("target/all_cov", exist_ok=True)
    merged = "target/all_cov/all.profdata"
    if os.path.exists(merged):
        os.remove(merged)
    if not profraws:
        print("No profraw files found!")
        sys.exit(1)
    subprocess.run([llvm_profdata, "merge", "-sparse"] + profraws + ["-o", merged], check=True)
    return merged

def report(merged_profdata, binaries):
    cov_args = [llvm_cov, "report", binaries[0]]
    for b in binaries[1:]:
        cov_args.extend(["-object", b])
    cov_args.extend([
        f"-instr-profile={merged_profdata}",
        r"-ignore-filename-regex=\.cargo|rustc|target|tests/|crates/[^/]+/tests/",
    ])
    res = subprocess.run(cov_args, capture_output=True, text=True)
    with open("target/all_cov/summary.txt", "w") as out:
        out.write(res.stdout)
    return res.stdout

def show_uncovered(file_path, merged_profdata, binaries):
    cov_args = [llvm_cov, "show", binaries[0]]
    for b in binaries[1:]:
        cov_args.extend(["-object", b])
    cov_args.extend([
        f"-instr-profile={merged_profdata}",
        file_path,
    ])
    res = subprocess.run(cov_args, capture_output=True, text=True)
    uncovered = []
    for l in res.stdout.splitlines():
        m = re.match(r"\s*(\d+)\|\s*(\d+)\|(.*)", l)
        if m:
            lineno, count, code = m.groups()
            if count == "0":
                uncovered.append((int(lineno), code))
    return uncovered

if __name__ == "__main__":
    action = sys.argv[1] if len(sys.argv) > 1 else "summary"
    if action == "run":
        run_tests()
        merged = merge_profdata()
        bins = get_binaries()
        print(report(merged, bins))
    elif action == "summary":
        merged = merge_profdata()
        bins = get_binaries()
        print(report(merged, bins))
    elif action == "uncovered":
        target_file = sys.argv[2]
        merged = "target/all_cov/all.profdata"
        bins = get_binaries()
        unc = show_uncovered(target_file, merged, bins)
        print(f"Uncovered lines in {target_file} ({len(unc)} lines):")
        for lineno, code in unc:
            print(f"{lineno:>4}: {code}")
