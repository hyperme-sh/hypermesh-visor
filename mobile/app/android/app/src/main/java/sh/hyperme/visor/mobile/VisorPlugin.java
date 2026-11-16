package sh.hyperme.visor.mobile;

import android.content.Intent;
import android.net.Uri;
import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.util.UUID;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import uniffi.hypermesh_visor_mobile.MobilePhoneClient;
import uniffi.hypermesh_visor_mobile.PhonePrompt;
import uniffi.hypermesh_visor_mobile.PreparedPhonePrompt;
import uniffi.hypermesh_visor_mobile.MachineStatus;
import uniffi.hypermesh_visor_mobile.MobileMachine;
import uniffi.hypermesh_visor_mobile.MobileOwnerSignIn;
import uniffi.hypermesh_visor_mobile.OwnerSignInPrompt;

/** Public state and specific operations only. No JS credential or URL bridge. */
@CapacitorPlugin(name = "Visor")
public final class VisorPlugin extends Plugin {
    private static NativeState shared;
    private NativeState nativeState;
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 0L, TimeUnit.SECONDS,
        new ArrayBlockingQueue<>(4), new ThreadPoolExecutor.AbortPolicy());

    private static final class NativeState {
        final String binding;
        final MobileMachine machine;
        final MobilePhoneClient phone;
        final MobileOwnerSignIn owner;
        final String workspaceId;
        final String workspaceName;
        NativeState(android.content.Context context, String issuer, String auth, String plane,
                    String workspace, String name, String conductor) throws Exception {
            if (!UUID.fromString(workspace).toString().equals(workspace) || name.isBlank() || name.length() > 128)
                throw new IllegalArgumentException();
            binding = String.join("\n", issuer, auth, plane, workspace, name, conductor == null ? "" : conductor);
            workspaceId = workspace;
            workspaceName = name;
            machine = AndroidSessionStoreKt.androidMachineIdentity(context, "device-owner-v1", plane);
            owner = AndroidSessionStoreKt.androidOwnerSignIn(context, "device-owner-v1", issuer, auth);
            phone = conductor == null ? null : new MobilePhoneClient(machine, conductor);
        }
    }

    @Override public void load() {
        try {
            String issuer = getConfig().getString("issuer");
            String auth = getConfig().getString("authOrigin");
            String plane = getConfig().getString("planeOrigin");
            String workspace = getConfig().getString("workspaceId");
            String name = getConfig().getString("workspaceName");
            String conductor = getConfig().getString("conductorOrigin");
            if (issuer == null || auth == null || plane == null || workspace == null || name == null) return;
            String binding = String.join("\n", issuer, auth, plane, workspace, name, conductor == null ? "" : conductor);
            synchronized (VisorPlugin.class) {
                if (shared == null) shared = new NativeState(getContext().getApplicationContext(), issuer, auth, plane, workspace, name, conductor);
                if (!shared.binding.equals(binding)) return;
                nativeState = shared;
            }
        } catch (Exception ignored) { /* No partial/plaintext fallback or raw diagnostics. */ }
    }

    private interface Action { JSObject run(NativeState state) throws Exception; }
    private void perform(PluginCall call, Action action) {
        if (nativeState == null) { call.reject("Phone configuration is unavailable."); return; }
        try {
            worker.execute(() -> {
                try { call.resolve(action.run(nativeState)); }
                catch (Exception ignored) { call.reject("That action could not be completed."); }
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) { call.reject("Another action is in progress."); }
    }
    private JSObject state(NativeState n) throws Exception {
        MachineStatus status = n.machine.status();
        JSObject result = new JSObject();
        result.put("enrolled", status != null && status.getEnrolled() && !status.getRevoked());
        result.put("revoked", status != null && status.getRevoked());
        result.put("connection", "disconnected");
        result.put("canConnect", false); // Infra #1747 has not supplied peer provisioning.
        return result;
    }
    @PluginMethod public void preparePrompt(PluginCall call) {
        perform(call, n -> {
            String bot = call.getString("botId"), assignment = call.getString("assignmentId"), session = call.getString("sessionId");
            String taskClass = call.getString("taskClass"), destination = call.getString("destination"), prompt = call.getString("prompt");
            Integer limit = call.getInt("maxOutputTokens");
            if (n.phone == null || bot == null || assignment == null || session == null || taskClass == null || destination == null || prompt == null || limit == null) throw new IllegalArgumentException();
            PreparedPhonePrompt prepared = n.phone.prepare(new PhonePrompt(bot, assignment, session, taskClass, destination, prompt, limit));
            JSObject result = new JSObject(); result.put("operationId", prepared.getOperationId());
            result.put("taskId", prepared.getTaskId()); result.put("requestSha256", prepared.getRequestSha256()); return result;
        });
    }
    @PluginMethod public void submitPrompt(PluginCall call) {
        perform(call, n -> {
            String id = call.getString("operationId");
            if (n.phone == null || id == null) throw new IllegalArgumentException();
            JSObject result = new JSObject(); result.put("text", n.phone.submit(id)); return result;
        });
    }
    @PluginMethod public void discardPrompt(PluginCall call) {
        perform(call, n -> {
            String id = call.getString("operationId");
            if (n.phone == null || id == null) throw new IllegalArgumentException();
            n.phone.discard(id); return new JSObject();
        });
    }
    @PluginMethod public void status(PluginCall call) { perform(call, this::state); }
    @PluginMethod public void beginSignIn(PluginCall call) {
        perform(call, n -> {
            OwnerSignInPrompt prompt = n.owner.begin();
            try {
                Intent browser = new Intent(Intent.ACTION_VIEW, Uri.parse(prompt.getVerificationUrl()));
                browser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                getContext().startActivity(browser);
            } catch (Exception failure) { n.owner.cancel(); throw failure; }
            JSObject result = new JSObject(); result.put("userCode", prompt.getUserCode()); return result;
        });
    }
    @PluginMethod public void finishSignIn(PluginCall call) {
        perform(call, n -> {
            n.owner.finish();
            JSObject candidate = new JSObject();
            candidate.put("id", n.workspaceId); candidate.put("name", n.workspaceName);
            JSObject result = new JSObject(); result.put("workspaces", new JSArray().put(candidate)); return result;
        });
    }
    @PluginMethod public void cancelSignIn(PluginCall call) {
        // Do not queue behind the blocking device-grant poll.
        try { if (nativeState != null) nativeState.owner.cancel(); call.resolve(); }
        catch (Exception ignored) { call.reject("Sign-in could not be cancelled."); }
    }
    @PluginMethod public void enroll(PluginCall call) {
        String selected = call.getString("workspaceId");
        perform(call, n -> {
            if (!n.workspaceId.equals(selected)) throw new IllegalArgumentException();
            MachineStatus prior = n.machine.status();
            if (prior != null && prior.getRevoked()) n.owner.revokeMachine(n.machine, prior.getTenantId());
            else if (prior != null && prior.getEnrolled()) throw new IllegalStateException();
            n.owner.enrollMachine(n.machine, n.workspaceId, "android");
            return state(n);
        });
    }
    @PluginMethod public void renew(PluginCall call) { perform(call, n -> { n.machine.renew(); return state(n); }); }
    @PluginMethod public void revoke(PluginCall call) {
        perform(call, n -> {
            MachineStatus current = n.machine.status();
            if (current == null) throw new IllegalStateException();
            n.owner.revokeMachine(n.machine, current.getTenantId()); return state(n);
        });
    }
    @PluginMethod public void connect(PluginCall call) { call.reject("Mesh provisioning is unavailable."); }
    @PluginMethod public void disconnect(PluginCall call) { perform(call, this::state); }
    @Override protected void handleOnDestroy() {
        try { if (nativeState != null) nativeState.owner.cancel(); } catch (Exception ignored) {}
        worker.shutdownNow();
    }
}
