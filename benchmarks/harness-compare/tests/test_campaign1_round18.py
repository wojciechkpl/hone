"""Round 18 (spec 11.19, K171-K175): hc-net is internal and isolated; preflight wants Docker 28.

`03_stack.sh` and `00_preflight.sh` run offline against a scripted fake `docker` (defined here,
opt-in; the shared fakebin helpers are unchanged). It logs every argv, then answers by env var:

* FAKE_NET_FORMAT  output of `docker network inspect -f '{{.Internal}} ...' hc-net` (the K172
  call, recognised by `.Internal` in the format); the literal `FAIL` makes the call exit 1
  (the network does not exist). Unset = FAIL.
* FAKE_GW          output of the gateway call (format contains `IPAM`); default empty.
* FAKE_GW_AFTER_CREATE  the gateway call's output once a `docker network create` of hc-net has
  been logged (default: same as FAKE_GW).
* FAKE_GW_FAIL  `1` makes the gateway call exit 1; FAKE_GW_FAIL_AFTER_CREATE does so only once
  hc-net has been created.
* FAKE_NET_CREATE_FAIL  `1` makes `docker network create` of hc-net exit 1 with an error.
* FAKE_STDIN_FILE  path where the probe's stdin (the in-container Python) is saved.
* FAKE_NET_RM_FAIL `1` makes `docker network rm` exit 1 with an error on stderr.
* FAKE_PROBE_OUT / FAKE_PROBE_RC  stdout and exit code of the probe's `docker run` (the one
  with `--network hc-net`); defaults `isolation ok` / 0.
* FAKE_DOCKER_VERSION  output of `docker version --format ...` (preflight).

Interface assumptions the implementer must honor:
* K172 reads the network with `docker network inspect -f <fmt> hc-net` where <fmt> is exactly
  the spec's string; any other `network inspect` call is answered with success and no output.
* the gateway call for the probe is a `network inspect` whose format mentions `IPAM`
  (as today); the probe passes it as `-e GW=<value>` on its `docker run --network hc-net`.
* the preflight Docker PASS line contains the server version and `28.0.0`.
"""

from __future__ import annotations

import re
import socket
import stat
import subprocess
import sys

import pytest
from fakebin import docker_calls, make_fake_bin, make_rig, run_script

FMT = (
    '{{.Internal}} {{index .Options "com.docker.network.bridge.gateway_mode_ipv4"}} '
    '{{index .Options "com.docker.network.bridge.gateway_mode_ipv6"}}'
)
GOOD = "true isolated isolated"
OLD_STYLE = "true  "

_DOCKER = """#!{python}
import json, os, sys
a = sys.argv[1:]
with open({log!r}, "a") as fh:
    fh.write(json.dumps(["docker"] + a) + "\\n")
env = os.environ.get
if a[:2] == ["network", "inspect"]:
    fmt = a[a.index("-f") + 1] if "-f" in a else ""
    if ".Internal" in fmt:
        out = env("FAKE_NET_FORMAT", "FAIL")
        if out == "FAIL":
            sys.stderr.write("Error: No such network\\n")
            sys.exit(1)
        print(out)
    elif "IPAM" in fmt:
        created = any(
            json.loads(ln)[1:3] == ["network", "create"] and "hc-net" in json.loads(ln)
            for ln in open({log!r}).read().splitlines() if ln.strip()
        )
        if env("FAKE_GW_FAIL") == "1" or (created and env("FAKE_GW_FAIL_AFTER_CREATE") == "1"):
            sys.stderr.write("Error: cannot inspect\\n")
            sys.exit(1)
        gw = env("FAKE_GW", "")
        print(env("FAKE_GW_AFTER_CREATE", gw) if created else gw)
elif a[:2] == ["network", "create"] and "hc-net" in a:
    if env("FAKE_NET_CREATE_FAIL") == "1":
        sys.stderr.write("Error response: cannot create network\\n")
        sys.exit(1)
elif a[:2] == ["network", "rm"]:
    if env("FAKE_NET_RM_FAIL") == "1":
        sys.stderr.write("Error response: network has active endpoints\\n")
        sys.exit(1)
elif a[:1] == ["run"] and "--network" in a and a[a.index("--network") + 1] == "hc-net":
    text = sys.stdin.read()
    if env("FAKE_STDIN_FILE"):
        open(env("FAKE_STDIN_FILE"), "w").write(text)
    print(env("FAKE_PROBE_OUT", "isolation ok"))
    sys.exit(int(env("FAKE_PROBE_RC", "0")))
elif a[:1] == ["version"]:
    print(env("FAKE_DOCKER_VERSION", "29.10.0"))
"""


def scripted_docker(tmp_path):
    """Fake bin dir with a scripted docker over the standard fakes; returns (bindir, log)."""
    bindir, log = make_fake_bin(tmp_path)
    path = bindir / "docker"
    path.write_text(_DOCKER.format(python=sys.executable, log=str(log)))
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    return bindir, log


def up(tmp_path, **env):
    rig = make_rig(tmp_path)
    bindir, log = scripted_docker(tmp_path)
    r = run_script(rig, "03_stack.sh", ["up"], bindir, env)
    return r, docker_calls(log)


def idx(cmds, pred):
    return [i for i, c in enumerate(cmds) if pred(c)]


def is_net_create(c):
    return c[:3] == ["network", "create", "--internal"] or (
        c[:2] == ["network", "create"] and "hc-net" in c
    )


def is_net_rm(c):
    return c[:3] == ["network", "rm", "hc-net"]


def is_proxy_run(c):
    return c[:1] == ["run"] and "hc-proxy" in c and "--name" in c


def option_pairs(c):
    return [c[i + 1] for i, a in enumerate(c) if a == "-o"]


def k172_inspects(cmds):
    return [c for c in cmds if c[:2] == ["network", "inspect"] and FMT in c]


# --- K171 ---------------------------------------------------------------------------------


def test_k171_up_should_create_hc_net_internal_with_isolated_gateway_modes(tmp_path):
    r, cmds = up(tmp_path)
    assert r.returncode == 0, r.stderr
    creates = [c for c in cmds if is_net_create(c)]
    assert len(creates) == 1
    c = creates[0]
    assert "--internal" in c and c[-1] == "hc-net"
    assert "com.docker.network.bridge.gateway_mode_ipv4=isolated" in option_pairs(c)
    assert "com.docker.network.bridge.gateway_mode_ipv6=isolated" in option_pairs(c)


# --- K172 ---------------------------------------------------------------------------------


def test_k172_up_should_read_hc_net_with_the_one_format_call(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=GOOD)
    assert r.returncode == 0, r.stderr
    found = k172_inspects(cmds)
    assert len(found) == 1 and found[0][-1] == "hc-net"


def test_k172_up_should_create_hc_net_when_the_inspect_call_fails(tmp_path):
    r, cmds = up(tmp_path)  # FAKE_NET_FORMAT unset: the network does not exist
    assert r.returncode == 0, r.stderr
    assert k172_inspects(cmds)
    assert not idx(cmds, is_net_rm)
    assert len(idx(cmds, is_net_create)) == 1


def test_k172_up_should_leave_a_correct_network_alone(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=GOOD)
    assert r.returncode == 0, r.stderr
    assert k172_inspects(cmds)
    assert not idx(cmds, is_net_rm)
    assert not idx(cmds, is_net_create)


def test_k172_up_should_replace_a_network_made_the_old_way(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE)
    assert r.returncode == 0, r.stderr
    rm, create = idx(cmds, is_net_rm), idx(cmds, is_net_create)
    assert len(rm) == 1 and len(create) == 1 and rm[0] < create[0]


def test_k172_up_should_replace_a_network_that_is_not_internal(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT="false isolated isolated")
    assert r.returncode == 0, r.stderr
    assert len(idx(cmds, is_net_rm)) == 1 and len(idx(cmds, is_net_create)) == 1


def test_k172_up_should_replace_a_network_isolated_for_ipv4_only(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT="true isolated ")
    assert r.returncode == 0, r.stderr
    assert len(idx(cmds, is_net_rm)) == 1 and len(idx(cmds, is_net_create)) == 1


def test_k172_replacement_should_come_after_rm_of_containers_and_before_the_proxy_starts(
    tmp_path,
):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE)
    assert r.returncode == 0, r.stderr
    rm_c = idx(cmds, lambda c: c[:3] == ["rm", "-f", "hc-proxy"] and "hc-sink" in c)
    rm_n, create = idx(cmds, is_net_rm), idx(cmds, is_net_create)
    proxy = idx(cmds, is_proxy_run)
    assert rm_c and rm_n and create and proxy
    assert rm_c[0] < rm_n[0] < create[0] < proxy[0]


def test_k172_creating_a_missing_network_should_also_come_before_the_proxy_starts(tmp_path):
    _, cmds = up(tmp_path)
    create, proxy = idx(cmds, is_net_create), idx(cmds, is_proxy_run)
    rm_c = idx(cmds, lambda c: c[:3] == ["rm", "-f", "hc-proxy"])
    assert rm_c and create and proxy and rm_c[0] < create[0] < proxy[0]


def last_error(r):
    lines = [ln for ln in r.stderr.splitlines() if "ERROR:" in ln]
    assert lines, r.stderr
    return lines[-1]


def test_k172_failed_network_rm_should_stop_up_with_an_error_naming_the_network(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE, FAKE_NET_RM_FAIL="1")
    assert r.returncode != 0
    err = last_error(r).lower()
    assert "hc-net" in err and "stop" in err and "container" in err
    rm = idx(cmds, is_net_rm)
    assert rm
    assert not [c for c in cmds[rm[0] + 1 :] if c[:1] == ["run"]]
    assert not idx(cmds, is_net_create)


def test_k172_with_a_campaign_1_key_missing_up_should_make_no_docker_call(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE, HC_TOP_K="")
    assert r.returncode != 0
    assert "HC_TOP_K" in r.stderr
    assert cmds == []


# --- K173 ---------------------------------------------------------------------------------


def probe_env_gw(tmp_path, gw):
    rig = make_rig(tmp_path)
    bindir, log = scripted_docker(tmp_path)
    r = run_script(rig, "03_stack.sh", ["probe"], bindir, {"FAKE_GW": gw})
    assert r.returncode == 0, r.stderr
    run = next(c for c in docker_calls(log) if c[:1] == ["run"] and "hc-net" in c)
    vals = [run[i + 1] for i, a in enumerate(run) if a == "-e" and run[i + 1].startswith("GW=")]
    assert len(vals) == 1
    return vals[0]


def test_k173_probe_should_pass_an_empty_gw_for_invalid_ip(tmp_path):
    assert probe_env_gw(tmp_path, "invalid IP") == "GW="


def test_k173_probe_should_pass_an_empty_gw_when_there_is_no_gateway(tmp_path):
    assert probe_env_gw(tmp_path, "") == "GW="


def test_k173_probe_should_pass_a_real_gateway_address_through(tmp_path):
    assert probe_env_gw(tmp_path, "172.19.0.1") == "GW=172.19.0.1"


# --- K174 ---------------------------------------------------------------------------------


def test_k174_up_error_should_carry_the_probe_findings_joined_by_semicolons(tmp_path):
    out = "host via gateway 172.19.0.1:22 reachable\nTCP to 1.1.1.1:443 works"
    r, _ = up(tmp_path, FAKE_PROBE_OUT=out, FAKE_PROBE_RC="1")
    assert r.returncode != 0
    assert (
        "network isolation check failed (host via gateway 172.19.0.1:22 reachable; "
        "TCP to 1.1.1.1:443 works) — do not run suite C on this host"
    ) in r.stderr


def test_k174_up_error_should_carry_a_single_finding_in_parentheses(tmp_path):
    r, _ = up(tmp_path, FAKE_PROBE_OUT="external DNS resolves (example.com)", FAKE_PROBE_RC="1")
    assert (
        "network isolation check failed (external DNS resolves (example.com)) "
        "— do not run suite C on this host"
    ) in r.stderr


def test_k174_probe_command_should_print_findings_and_return_the_probe_exit_code(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = scripted_docker(tmp_path)
    env = {"FAKE_PROBE_OUT": "TCP to 8.8.8.8:443 works", "FAKE_PROBE_RC": "1"}
    r = run_script(rig, "03_stack.sh", ["probe"], bindir, env)
    assert r.returncode == 1
    assert "TCP to 8.8.8.8:443 works" in r.stdout


# --- K175 ---------------------------------------------------------------------------------


def docker_version_lines(tmp_path, version):
    rig = make_rig(tmp_path)
    bindir, _ = scripted_docker(tmp_path)
    r = run_script(rig, "00_preflight.sh", [], bindir, {"FAKE_DOCKER_VERSION": version})
    return [ln for ln in r.stdout.splitlines() if f"docker {version}" in ln]


def test_k175_preflight_should_fail_below_28_naming_the_version_and_isolated(tmp_path):
    lines = docker_version_lines(tmp_path, "27.5.1")
    assert len(lines) == 1
    assert "FAIL" in lines[0]
    assert "28.0.0" in lines[0] and "isolated" in lines[0]


def test_k175_preflight_should_fail_for_the_old_25_0_5_minimum(tmp_path):
    lines = docker_version_lines(tmp_path, "25.0.5")
    assert len(lines) == 1 and "FAIL" in lines[0]
    assert "28.0.0" in lines[0] and "isolated" in lines[0]


def test_k175_preflight_should_pass_at_exactly_28_0_0(tmp_path):
    lines = docker_version_lines(tmp_path, "28.0.0")
    assert len(lines) == 1
    assert "PASS" in lines[0] and "25.0.5" not in lines[0]  # the 25.0.5 check is replaced


def test_k175_preflight_should_pass_above_28(tmp_path):
    lines = docker_version_lines(tmp_path, "29.10.0")
    assert len(lines) == 1
    assert "PASS" in lines[0] and "28.0.0" in lines[0]


# --- review round: K171 on the replace path ------------------------------------------------


def test_k171_the_create_after_a_network_rm_should_carry_internal_and_both_isolated_options(
    tmp_path,
):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE)
    assert r.returncode == 0, r.stderr
    rm = idx(cmds, is_net_rm)
    create = [c for c in cmds[rm[0] + 1 :] if is_net_create(c)]
    assert len(create) == 1
    c = create[0]
    assert "--internal" in c and c[-1] == "hc-net"
    assert "com.docker.network.bridge.gateway_mode_ipv4=isolated" in option_pairs(c)
    assert "com.docker.network.bridge.gateway_mode_ipv6=isolated" in option_pairs(c)


# --- K174 fallback -------------------------------------------------------------------------


def test_k174_a_silent_failing_probe_should_still_leave_text_in_the_brackets(tmp_path):
    r, _ = up(tmp_path, FAKE_PROBE_OUT="", FAKE_PROBE_RC="1")
    assert r.returncode != 0
    m = re.search(r"network isolation check failed \((.+)\) — do not run suite C", last_error(r))
    assert m and m.group(1).strip()


# --- K177 ----------------------------------------------------------------------------------


def test_k177_gateway_call_should_use_the_format_with_a_space_after_each_value(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = scripted_docker(tmp_path)
    run_script(rig, "03_stack.sh", ["probe"], bindir, {})
    gw_calls = [
        c for c in docker_calls(log) if c[:2] == ["network", "inspect"] and "IPAM" in " ".join(c)
    ]
    assert gw_calls
    assert all(
        c[c.index("-f") + 1] == "{{range .IPAM.Config}}{{.Gateway}} {{end}}" for c in gw_calls
    )


def test_k177_gw_should_be_empty_for_invalid_ip_with_a_trailing_space(tmp_path):
    assert probe_env_gw(tmp_path, "invalid IP ") == "GW="


def test_k177_gw_should_hold_an_ipv4_gateway_with_a_trailing_space(tmp_path):
    assert probe_env_gw(tmp_path, "172.19.0.1 ") == "GW=172.19.0.1"


def test_k177_gw_should_hold_both_gateways_of_a_dual_stack_network(tmp_path):
    out = probe_env_gw(tmp_path, "172.23.0.1 fd1c:ab34:fe4b::1 ")
    assert out == "GW=172.23.0.1 fd1c:ab34:fe4b::1"


def test_k177_gw_should_keep_only_the_ipv6_address_when_ipv4_has_no_gateway(tmp_path):
    out = probe_env_gw(tmp_path, "invalid IP fd1c:ab34:fe4b::1 ")
    assert out == "GW=fd1c:ab34:fe4b::1"


def test_k177_ports_should_still_start_with_22_80_443(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = scripted_docker(tmp_path)
    run_script(rig, "03_stack.sh", ["probe"], bindir, {})
    run = next(c for c in docker_calls(log) if c[:1] == ["run"] and "hc-net" in c)
    ports = next(run[i + 1] for i, a in enumerate(run) if a == "-e" and run[i + 1][:6] == "PORTS=")
    assert ports.startswith("PORTS=22 80 443 ")


def run_probe_python(tmp_path, gw, ports):
    """Run the Python the script feeds the probe container on stdin, locally (not in a box)."""
    rig = make_rig(tmp_path)
    bindir, _ = scripted_docker(tmp_path)
    saved = tmp_path / "probe_stdin.py"
    run_script(rig, "03_stack.sh", ["probe"], bindir, {"FAKE_STDIN_FILE": str(saved)})
    assert saved.read_text().strip(), "the probe fed nothing on stdin"
    env = {"GW": gw, "PORTS": ports, "PROXY_PORT": "9", "PATH": "/usr/bin:/bin"}
    return subprocess.run(
        [sys.executable, "-I", str(saved)],
        capture_output=True,
        text=True,
        env=env,
        cwd=tmp_path,
        timeout=60,
        check=False,
    )


def test_k177_container_should_try_the_ports_on_every_address_in_gw(tmp_path):
    v4 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    v4.bind(("127.0.0.1", 0))
    port = v4.getsockname()[1]
    v6 = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
    try:
        v6.bind(("::1", port))
    except OSError:
        v4.close()
        v6.close()
        pytest.skip("::1 cannot be bound on this host")
    v4.listen()
    v6.listen()
    try:
        r = run_probe_python(tmp_path, "127.0.0.1 ::1", str(port))
    finally:
        v4.close()
        v6.close()
    assert f"host via gateway 127.0.0.1:{port} reachable" in r.stdout
    assert f"host via gateway [::1]:{port} reachable" in r.stdout


def test_k177_container_should_try_no_host_port_when_gw_is_empty(tmp_path):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen()
    try:
        r = run_probe_python(tmp_path, "", str(srv.getsockname()[1]))
    finally:
        srv.close()
    assert "host via gateway" not in r.stdout


# --- K178 ----------------------------------------------------------------------------------


def test_k178_a_network_with_the_right_options_but_a_gateway_address_should_be_replaced(
    tmp_path,
):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=GOOD, FAKE_GW="172.19.0.1 ", FAKE_GW_AFTER_CREATE="")
    assert r.returncode == 0, r.stderr
    rm, create = idx(cmds, is_net_rm), idx(cmds, is_net_create)
    assert len(rm) == 1 and len(create) == 1 and rm[0] < create[0]


def test_k178_a_correct_network_with_no_gateway_address_should_be_left_alone(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=GOOD, FAKE_GW="invalid IP ")
    assert r.returncode == 0, r.stderr
    assert not idx(cmds, is_net_rm) and not idx(cmds, is_net_create)


def test_k178_a_gateway_after_create_should_stop_up_naming_isolated_and_28(tmp_path):
    r, _ = up(tmp_path, FAKE_GW_AFTER_CREATE="172.19.0.1 ")
    assert r.returncode != 0
    err = last_error(r)
    assert "isolated" in err and "28.0.0" in err


def test_k178_a_gateway_after_create_should_remove_the_network_and_run_nothing(tmp_path):
    _, cmds = up(tmp_path, FAKE_GW_AFTER_CREATE="172.19.0.1 ")
    create = idx(cmds, is_net_create)
    assert len(create) == 1
    assert [i for i in idx(cmds, is_net_rm) if i > create[0]]
    assert not [c for c in cmds if c[:1] == ["run"]]


# --- K179 ----------------------------------------------------------------------------------


def test_k179_failed_network_create_should_stop_up_naming_the_network_without_a_version(
    tmp_path,
):
    r, cmds = up(tmp_path, FAKE_NET_CREATE_FAIL="1")
    assert r.returncode != 0
    err = last_error(r)
    assert "hc-net" in err
    assert not re.search(r"\d+\.\d+", err)
    assert idx(cmds, is_net_create)
    assert not [c for c in cmds if c[:1] == ["run"]]


def test_k179_failed_network_rm_should_show_docker_s_own_reason(tmp_path):
    r, _ = up(tmp_path, FAKE_NET_FORMAT=OLD_STYLE, FAKE_NET_RM_FAIL="1")
    assert r.returncode != 0
    assert "network has active endpoints" in r.stderr


# --- K181 ----------------------------------------------------------------------------------


def no_run(cmds):
    return not [c for c in cmds if c[:1] == ["run"]]


def test_k181_failed_gateway_read_on_an_existing_network_should_stop_up(tmp_path):
    r, cmds = up(tmp_path, FAKE_NET_FORMAT=GOOD, FAKE_GW_FAIL="1")
    assert r.returncode != 0
    err = last_error(r)
    assert "hc-net" in err and "gateway" in err
    assert no_run(cmds)


def test_k181_failed_gateway_read_after_a_create_should_stop_up(tmp_path):
    r, cmds = up(tmp_path, FAKE_GW_FAIL_AFTER_CREATE="1")
    assert r.returncode != 0
    assert idx(cmds, is_net_create)
    err = last_error(r)
    assert "hc-net" in err and "gateway" in err
    assert no_run(cmds)


def test_k181_probe_should_say_it_cannot_read_the_gateway_and_start_nothing(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = scripted_docker(tmp_path)
    r = run_script(rig, "03_stack.sh", ["probe"], bindir, {"FAKE_GW_FAIL": "1"})
    assert r.returncode == 1
    assert "cannot read the gateway of hc-net" in r.stdout
    assert no_run(docker_calls(log))
