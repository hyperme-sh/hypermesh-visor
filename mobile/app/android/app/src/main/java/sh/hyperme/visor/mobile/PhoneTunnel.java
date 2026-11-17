package sh.hyperme.visor.mobile;

import android.content.Context;
import android.content.Intent;
import com.wireguard.config.Config;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import sh.hyperme.visor.mobile.tunnel.AndroidMeshTunnel;
import uniffi.hypermesh_visor_mobile.MobileMachine;
import uniffi.hypermesh_visor_mobile.MobileMeshClient;
import uniffi.hypermesh_visor_mobile.MobileMeshLease;
import uniffi.hypermesh_visor_mobile.MobileOwnerSignIn;
import uniffi.hypermesh_visor_mobile.MobilePhoneClient;

/** In-memory configuration only; no VPN key or machine credential crosses the JS bridge. */
final class PhoneTunnel {
    final MobileMeshClient mesh;
    private final MobileMachine machine;
    private final MobileOwnerSignIn owner;
    private final AndroidMeshTunnel tunnel;
    private final String packageName;
    private final String ca;
    private final ScheduledExecutorService refresh = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledExecutorService expiry = Executors.newSingleThreadScheduledExecutor();
    private MobileMeshLease lease;
    private MobilePhoneClient phone;
    private String configuration;
    private String conductor;
    private long generation;
    private boolean wanted;

    PhoneTunnel(Context context, MobileMachine machine, MobileOwnerSignIn owner, String ca) {
        this.machine = machine; this.owner = owner; this.ca = ca;
        mesh = new MobileMeshClient(machine);
        tunnel = new AndroidMeshTunnel(context); packageName = context.getPackageName();
        refresh.scheduleWithFixedDelay(this::renew, 20, 20, TimeUnit.SECONDS);
        expiry.scheduleWithFixedDelay(this::expire, 1, 1, TimeUnit.SECONDS);
    }
    Intent permissionRequest() { return tunnel.permissionRequest(); }
    synchronized long begin() { disconnect(); wanted = true; return generation; }
    void connect(long expected) throws Exception {
        synchronized (this) { if (!wanted || generation != expected) return; }
        MobileMeshLease next = mesh.prepare(owner);
        String raw = next.wireguardConfiguration();
        String origin = next.conductorOrigin();
        synchronized (this) {
            if (!wanted || generation != expected || !next.valid()) return;
            try {
                if (!raw.equals(configuration)) {
                    String scoped = raw.replace("[Interface]\n", "[Interface]\nIncludedApplications = " + packageName + "\n");
                    Config config = Config.parse(new ByteArrayInputStream(scoped.getBytes(StandardCharsets.UTF_8)));
                    if (!tunnel.start(config)) throw new IllegalStateException();
                    phone = new MobilePhoneClient(machine, origin, ca);
                } else if (!origin.equals(conductor)) {
                    phone = new MobilePhoneClient(machine, origin, ca);
                }
                lease = next; configuration = raw; conductor = origin;
            } catch (Exception failure) { disconnect(); throw failure; }
        }
    }
    private void renew() {
        long expected;
        synchronized (this) { if (!wanted || lease == null) return; expected = generation; }
        try { connect(expected); }
        catch (Exception ignored) { synchronized (this) { if (generation == expected) disconnect(); } }
    }
    private synchronized void expire() {
        try { if (lease != null && !lease.valid()) disconnect(); }
        catch (Exception ignored) { disconnect(); }
    }
    synchronized String status() {
        expire();
        return wanted ? tunnel.status().name().toLowerCase(Locale.ROOT) : "disconnected";
    }
    synchronized MobilePhoneClient client() {
        if (!"connected".equals(status()) || phone == null) throw new IllegalStateException();
        return phone;
    }
    synchronized void failed(long expected) { if (generation == expected) disconnect(); }
    synchronized void disconnect() {
        generation++; wanted = false; mesh.cancel(); owner.cancelPhoneOperations();
        tunnel.stop(); lease = null; phone = null; configuration = null; conductor = null;
    }
    void close() { disconnect(); refresh.shutdownNow(); expiry.shutdownNow(); }
}
