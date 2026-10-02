#!/usr/bin/env python3
"""Manage only INPUT rules for this deployment's two internal Docker bridges.

Run after `docker compose create` and before starting containers. Requires local
rootful Linux Docker and root/noninteractive sudo. Never changes default policy,
FORWARD rules, other projects, or unverified network interfaces.
"""
import argparse
import json
import os
import re
import shlex
import subprocess
import sys


def run(command, check=True):
    result = subprocess.run(command, capture_output=True, text=True, timeout=20)
    if check and result.returncode:
        raise RuntimeError(result.stderr.strip() or 'command failed')
    return result


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def docker(*args):
    return run(['docker', *args]).stdout


def firewall(action, rule, check=True):
    prefix = [] if os.geteuid() == 0 else ['sudo', '-n']
    return run(prefix + ['iptables', '-w', '5', action, 'INPUT', *rule], check=check)


def verify_precedence(rules):
    prefix = [] if os.geteuid() == 0 else ['sudo', '-n']
    listing = run(prefix + ['iptables', '-w', '5', '-S', 'INPUT']).stdout
    remaining = {tuple(['-A', 'INPUT', *rule]) for rule in rules}
    for line in listing.splitlines():
        tokens = shlex.split(line)
        if not tokens or tokens[:2] == ['-P', 'INPUT']:
            continue
        require(tokens[:2] == ['-A', 'INPUT'], 'unexpected INPUT firewall listing')
        # iptables may render the implicit IPv4 REJECT type explicitly.
        normalized = tokens
        if tokens[-2:] == ['--reject-with', 'icmp-port-unreachable']:
            normalized = tokens[:-2]
        remaining.discard(tuple(normalized))
        if not remaining:
            return
        # Conditional DROP/REJECT can only deny or continue; other targets may
        # accept via a chain, so do not infer their effects. Never renumber rules.
        jump = tokens.index('-j') if '-j' in tokens else -1
        terminal_deny = (jump >= 0 and jump + 1 < len(tokens)
                         and tokens[jump + 1] in ['DROP', 'REJECT']
                         and '-g' not in tokens and '--goto' not in tokens)
        require(terminal_deny,
                'a potentially accepting INPUT rule precedes the project guard; '
                'stop all project containers, remove its guard, then apply again')
    require(not remaining, 'project INPUT guard rules are missing from the firewall listing')


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('action', choices=['apply', 'remove'])
parser.add_argument('--project', required=True)
args = parser.parse_args()
require(sys.platform.startswith('linux'), 'requires native Linux')
require(re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,62}', args.project), 'invalid Compose project name')
endpoint = os.environ.get('DOCKER_HOST') or json.loads(docker('context', 'inspect', '--format', '{{json .Endpoints.docker.Host}}'))
require(endpoint.startswith('unix://'), 'Docker daemon must run on this host')
info = json.loads(docker('info', '--format', '{{json .}}'))
require(info['OSType'] == 'linux' and 'rootless' not in str(info.get('SecurityOptions', [])), 'requires rootful Linux Docker')
rules = []
for tenant in ['alice', 'bob']:
    name = args.project + '_' + tenant + '_private'
    network = json.loads(docker('network', 'inspect', name))[0]
    require(network['Driver'] == 'bridge' and network['Internal'] and not network['EnableIPv6'], 'requires internal IPv4 bridge')
    require(network['Labels'].get('com.docker.compose.project') == args.project, 'network belongs to another project')
    bridge = network.get('Options', {}).get('com.docker.network.bridge.name') or 'br-' + network['Id'][:12]
    require(re.fullmatch(r'[a-zA-Z0-9_.-]{1,15}', bridge), 'invalid bridge interface name')
    rules.append(['-i', bridge, '-m', 'comment', '--comment',
                  'jiaclaw-gateway:' + args.project + ':' + tenant, '-j', 'REJECT'])
added = []
try:
    for rule in rules:
        existing = firewall('-C', rule, check=False)
        if existing.returncode not in [0, 1]:
            raise RuntimeError('cannot inspect INPUT firewall rules: ' + existing.stderr.strip())
        if args.action == 'apply' and existing.returncode == 1:
            firewall('-I', rule)
            added.append(rule)
        elif args.action == 'remove' and existing.returncode == 0:
            firewall('-D', rule)
    if args.action == 'apply':
        verify_precedence(rules)
except Exception:
    for rule in reversed(added):
        firewall('-D', rule, check=False)
    raise
print('Applied host-service isolation' if args.action == 'apply' else 'Removed this deployment host-service isolation')
