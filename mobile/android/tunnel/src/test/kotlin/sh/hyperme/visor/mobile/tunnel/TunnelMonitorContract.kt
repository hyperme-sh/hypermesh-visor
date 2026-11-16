package sh.hyperme.visor.mobile.tunnel

fun tunnelMonitorContract() {
    val monitor = TunnelMonitor()
    check(monitor.sample(true, 1_001, 1_001, 1) == TunnelStatus.DISCONNECTED)
    monitor.begin(10_000, 100)
    check(monitor.sample(true, 9_999, 10_001, 101) == TunnelStatus.CONNECTING)
    check(monitor.sample(true, 99_999, 10_002, 102) == TunnelStatus.UNREACHABLE)
    check(monitor.sample(true, 10_001, 10_003, 103) == TunnelStatus.CONNECTED)
    check(monitor.sample(true, 0, 10_004, 104) == TunnelStatus.RECONNECTING)
    check(monitor.sample(true, 10_001, 190_002, 180_104) == TunnelStatus.RECONNECTING)
    check(monitor.sample(true, 190_003, 190_004, 180_105) == TunnelStatus.CONNECTED)
    check(monitor.sample(false, 190_003, 190_005, 180_106) == TunnelStatus.UNREACHABLE)
    check(monitor.sample(true, 190_003, 190_006, 180_107) != TunnelStatus.CONNECTED)
    monitor.stop()
    check(monitor.sample(true, 190_007, 190_007, 180_108) == TunnelStatus.DISCONNECTED)
    monitor.begin(200_000, 200_000)
    check(monitor.sample(true, 0, 230_001, 230_001) == TunnelStatus.UNREACHABLE)
    monitor.begin(300_000, 300_000)
    check(monitor.sample(true, 300_001, 300_001, 300_001) == TunnelStatus.CONNECTED)
    // Frozen wall time cannot keep an old handshake fresh indefinitely.
    check(monitor.sample(true, 300_001, 300_001, 480_002) == TunnelStatus.RECONNECTING)
    check(monitor.sample(true, 300_001, 300_000, 480_003) == TunnelStatus.UNREACHABLE)
    check(monitor.sample(true, 500_000, 500_001, 500_001) == TunnelStatus.UNREACHABLE)
    monitor.begin(600_000, 600_000)
    check(monitor.sample(true, 600_001, 600_001, 599_999) == TunnelStatus.UNREACHABLE)
    monitor.begin(650_500, 650_500)
    check(monitor.sample(true, 650_000, 650_501, 650_501) == TunnelStatus.CONNECTED)
    monitor.begin(1_000_000, 1_000_000)
    check(monitor.sample(true, 1_000_000, 1_100_000, 1_100_000) == TunnelStatus.CONNECTED)
    check(monitor.sample(true, 1_000_000, 1_100_000, 1_180_001) == TunnelStatus.RECONNECTING)
    monitor.begin(1_000_000, 0)
    check(monitor.sample(true, 1_600_000, 1_600_000, 1_000) == TunnelStatus.CONNECTED)
    check(monitor.sample(true, 1_600_000, 1_600_000, 301_000) == TunnelStatus.RECONNECTING)
    monitor.failed()
    check(monitor.sample(false, 0, 700_000, 700_000) == TunnelStatus.UNREACHABLE)
}

fun main() { tunnelMonitorContract(); println("Tunnel monitor contract passed") }
