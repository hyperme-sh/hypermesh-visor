"""Exercise process identity checks without starting a display or using sudo."""
import pathlib
import re
import subprocess

source = pathlib.Path(__file__).with_name('run.sh').read_text()
functions = '\n'.join(
    re.search(r'^' + name + r'\(\) \{\n.*?^\}', source, re.M | re.S).group()
    for name in ('proc_executable', 'exe_is_xorg', 'pid_matches_exe')
)
subprocess.run(['bash', '-eu', '-c', functions + r'''
# Simulate a root-owned Xorg: unprivileged readlink fails, while a narrowly
# scoped noninteractive sudo read succeeds. cmdline fallback is unavailable.
readlink() { return 1; }
sudo() {
  [[ "$*" == '-n readlink /proc/123/exe' ]] || return 1
  printf '%s\n' /usr/lib/xorg/Xorg
}
[[ $(proc_executable 123) == /usr/lib/xorg/Xorg ]]
exe_is_xorg 123
! exe_is_xorg 456
! proc_executable '../123'
# An ordinary user-owned process must not invoke sudo.
readlink() { printf '%s\n' /usr/bin/bash; }
sudo() { echo 'unexpected sudo' >&2; exit 99; }
[[ $(proc_executable $$) == /usr/bin/bash ]]
pid_matches_exe $$ /usr/bin/bash
! pid_matches_exe $$ /usr/lib/xorg/Xorg
! exe_is_xorg $$
'''], check=True)
print('process identity checks passed')

# Readiness diagnostics use command stubs only; no X server or sudo.
readiness = '\n'.join(
    re.search(r'^' + name + r'\(\) \{\n.*?^\}', source, re.M | re.S).group()
    for name in ('xorg_failure', 'wait_for_display')
)
result = subprocess.run(['bash', '-eu', '-c', readiness + r'''
export DISPLAY=:99
sleep() { :; }
xdpyinfo() { return 1; }
wait_for_display
'''], text=True, capture_output=True)
assert result.returncode == 1
assert result.stderr == 'Xorg startup stage: display-unreachable\n'
result = subprocess.run(['bash', '-eu', '-c', readiness + r'''
command() { return 1; }
wait_for_display
'''], text=True, capture_output=True)
assert result.returncode == 1
assert result.stderr == 'Xorg startup stage: display-probe-missing\n'
result = subprocess.run(['bash', '-eu', '-c', readiness + "\nxorg_failure private-token-sentinel"],
                        text=True, capture_output=True)
assert result.returncode == 2 and not result.stderr
# Reproduce an exited /proc entry with the actual cleanup read expression.
read = re.search(r"cmd=\$\(tr .*?/proc/\$pid/cmdline.*?\)", source).group()
result = subprocess.run(['bash', '-eu', '-c', 'pid=999999999\n'+read],
                        text=True, capture_output=True)
assert result.returncode == 0 and not result.stderr
print('startup diagnostics and exited-process cleanup checks passed')
