import os, pty, subprocess, tempfile, termios, time, json
from pathlib import Path
binary=str(Path(__file__).resolve().parents[2]/'target/debug/hypermesh-companion')
with tempfile.TemporaryDirectory() as d:
    mock=Path(d)/'hypermesh'
    mock.write_text('#!/usr/bin/env python3\nimport os,sys,json\nopen(os.environ["PROMPT_LOG"],"w").write(json.dumps([sys.stdin.read(),os.environ.get("HYPERMESH_VISOR_URL")]))\n')
    mock.chmod(0o700)
    for size in (5000,17000):
        master, slave=pty.openpty()
        log=Path(d)/str(size)
        env=dict(os.environ,PATH=d+':'+os.environ['PATH'],PROMPT_LOG=str(log),HYPERMESH_VISOR_URL='http://wrong.invalid')
        proc=subprocess.Popen([binary,'--session-cli','01234567-89ab-cdef-0123-456789abcdef'],stdin=slave,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,env=env)
        for _ in range(200):
            if not termios.tcgetattr(slave)[3] & termios.ICANON: break
            time.sleep(.01)
        else: raise AssertionError('terminal mode unchanged')
        data=(('x'*size)+'\n/quit\n').encode()
        while data:
            sent=os.write(master,data[:1024]); data=data[sent:]
        proc.communicate(timeout=15)
        if size==5000:
            assert proc.returncode==0
            assert json.loads(log.read_text())==['x'*size,None]
        else:
            assert proc.returncode==1 and not log.exists()
        assert termios.tcgetattr(slave)[3] & termios.ICANON
        os.close(master);os.close(slave)
print('PTY contracts passed: 5000 bytes intact, 17000 bytes refused, local endpoint, terminal restored')
