package sh.hyperme.visor.mobile;

import android.content.Intent;
import android.app.Activity;
import androidx.activity.result.ActivityResult;
import com.getcapacitor.annotation.ActivityCallback;
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
import uniffi.hypermesh_visor_mobile.MobileFeedbackClient;
import uniffi.hypermesh_visor_mobile.PhonePrompt;
import uniffi.hypermesh_visor_mobile.PreparedPhonePrompt;
import uniffi.hypermesh_visor_mobile.MachineStatus;
import uniffi.hypermesh_visor_mobile.MobileMachine;
import uniffi.hypermesh_visor_mobile.MobileOwnerSignIn;
import uniffi.hypermesh_visor_mobile.OwnerSignInPrompt;

/** Public state and specific operations only. No JS credential or URL bridge. */
@CapacitorPlugin(name = "Visor")
public final class VisorPlugin extends Plugin {
    private NativeState nativeState;
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 0L, TimeUnit.SECONDS,
        new ArrayBlockingQueue<>(4), new ThreadPoolExecutor.AbortPolicy());

    private static final class NativeState {
        final String binding;
        final MobileMachine machine;
        MobileFeedbackClient feedback;
        final PhoneTunnel tunnel;
        final MobileOwnerSignIn owner;
        final String workspaceId;
        final String workspaceName;
        NativeState(android.content.Context context, String issuer, String auth, String plane,
                    String workspace, String name, String conductor, String ca) throws Exception {
            if (!UUID.fromString(workspace).toString().equals(workspace) || name.isBlank() || name.length() > 128)
                throw new IllegalArgumentException();
            binding = String.join("\n", issuer, auth, plane, workspace, name, conductor == null ? "" : conductor);
            workspaceId = workspace;
            workspaceName = name;
            machine = AndroidSessionStoreKt.androidMachineIdentity(context, "device-owner-v1", plane);
            owner = AndroidSessionStoreKt.androidOwnerSignIn(context, "device-owner-v1", issuer, auth);
            tunnel = new PhoneTunnel(context, machine, owner, ca);
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
            nativeState = new NativeState(getContext().getApplicationContext(), issuer, auth, plane, workspace, name, conductor, getConfig().getString("conductorCaPem"));
            if (getConfig().getBoolean("feedbackEnabled", false)) {
                String intake = getConfig().getString("feedbackIntakeOrigin");
                if (intake != null) nativeState.feedback = new MobileFeedbackClient(nativeState.machine, intake);
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
        } catch (java.util.concurrent.RejectedExecutionException ignored) { call.reject("Another action is in progress.", "BUSY"); }
    }
    private JSObject state(NativeState n) throws Exception {
        MachineStatus status = n.machine.status();
        JSObject result = new JSObject();
        result.put("enrolled", status != null && status.getEnrolled() && !status.getRevoked());
        result.put("revoked", status != null && status.getRevoked());
        result.put("connection", n.tunnel.status());
        result.put("canConnect", status != null && status.getEnrolled() && !status.getRevoked());
        return result;
    }
    @PluginMethod public void feedbackStatus(PluginCall call) {
        JSObject result = new JSObject(); result.put("configured", nativeState != null && nativeState.feedback != null); call.resolve(result);
    }
    private MobileFeedbackClient feedback(NativeState n) {
        if (n.feedback == null) throw new IllegalStateException(); return n.feedback;
    }
    private String feedbackSession(PluginCall call) {
        String id = call.getString("sessionId"); if (id == null) throw new IllegalArgumentException(); return id;
    }
    @PluginMethod public void feedbackStart(PluginCall call) {
        try {
            if (nativeState == null || !Boolean.TRUE.equals(call.getBoolean("optedIn"))) throw new IllegalStateException();
            JSObject result = new JSObject(); result.put("sessionId", feedback(nativeState).start(true)); call.resolve(result);
        } catch (Exception ignored) { call.reject("Feedback unavailable."); }
    }
    @PluginMethod public void feedbackCancel(PluginCall call) {
        try { if (nativeState != null && nativeState.feedback != null) nativeState.feedback.cancel(feedbackSession(call)); call.resolve(); }
        catch (Exception ignored) { call.reject("Feedback unavailable."); }
    }
    @PluginMethod public void feedbackConsent(PluginCall call) {
        perform(call, n -> { JSObject out = new JSObject(); out.put("enabled", feedback(n).consent(feedbackSession(call))); return out; });
    }
    @PluginMethod public void feedbackSetConsent(PluginCall call) {
        perform(call, n -> { Boolean enabled = call.getBoolean("enabled"); if (enabled == null) throw new IllegalArgumentException(); feedback(n).setConsent(feedbackSession(call), enabled); return new JSObject(); });
    }
    @PluginMethod public void feedbackPrepare(PluginCall call) {
        perform(call, n -> {
            String kind = call.getString("kind"), title = call.getString("title"), description = call.getString("description");
            if (kind == null || title == null || description == null) throw new IllegalArgumentException();
            var preview = feedback(n).prepare(feedbackSession(call), kind, title, description);
            JSObject out = new JSObject(); out.put("operationId", preview.getOperationId()); out.put("kind", preview.getKind()); out.put("title", preview.getTitle()); out.put("description", preview.getDescription()); return out;
        });
    }
    @PluginMethod public void feedbackSubmit(PluginCall call) {
        perform(call, n -> { String op = call.getString("operationId"); if (op == null) throw new IllegalArgumentException(); var sent = feedback(n).submitReport(feedbackSession(call), op); JSObject out = new JSObject(); out.put("submitted", sent.getSubmitted()); out.put("retryable", sent.getRetryable()); return out; });
    }
    private void clearFeedback() { if (nativeState != null && nativeState.feedback != null) nativeState.feedback.cancelAll(); }
    @Override protected void handleOnPause() { clearFeedback(); super.handleOnPause(); }
    @PluginMethod public void cancelPhone(PluginCall call) {
        if (nativeState != null) nativeState.owner.cancelPhoneOperations();
        call.resolve();
    }
    @PluginMethod public void ownedSessions(PluginCall call) {
        perform(call, n -> {
            JSArray sessions = new JSArray();
            for (var session : n.tunnel.mesh.ownedSessions(n.owner)) {
                JSObject item = new JSObject(); item.put("botId", session.getBotId());
                item.put("assignmentId", session.getAssignmentId()); item.put("sessionId", session.getSessionId());
                item.put("expiresAt", session.getExpiresAt()); sessions.put(item);
            }
            JSObject result = new JSObject(); result.put("sessions", sessions); return result;
        });
    }
    @PluginMethod public void sendPrompt(PluginCall call) {
        perform(call, n -> {
            String bot = call.getString("botId"), assignment = call.getString("assignmentId"), session = call.getString("sessionId");
            String taskClass = call.getString("taskClass"), destination = call.getString("destination"), prompt = call.getString("prompt");
            Integer limit = call.getInt("maxOutputTokens");
            long expires = call.getData().getLong("expiresAt");
            if (bot == null || assignment == null || session == null || taskClass == null || destination == null || prompt == null || limit == null) throw new IllegalArgumentException();
            JSObject result = new JSObject(); result.put("text", n.tunnel.client().sendOwned(n.owner,
                new PhonePrompt(bot, assignment, session, taskClass, destination, prompt, limit), expires)); return result;
        });
    }
    @PluginMethod public void preparePrompt(PluginCall call) {
        perform(call, n -> {
            String bot = call.getString("botId"), assignment = call.getString("assignmentId"), session = call.getString("sessionId");
            String taskClass = call.getString("taskClass"), destination = call.getString("destination"), prompt = call.getString("prompt");
            Integer limit = call.getInt("maxOutputTokens");
            if (bot == null || assignment == null || session == null || taskClass == null || destination == null || prompt == null || limit == null) throw new IllegalArgumentException();
            PreparedPhonePrompt prepared = n.tunnel.client().prepare(new PhonePrompt(bot, assignment, session, taskClass, destination, prompt, limit));
            JSObject result = new JSObject(); result.put("operationId", prepared.getOperationId());
            result.put("taskId", prepared.getTaskId()); result.put("requestSha256", prepared.getRequestSha256()); return result;
        });
    }
    @PluginMethod public void submitPrompt(PluginCall call) {
        perform(call, n -> {
            String id = call.getString("operationId");
            if (id == null) throw new IllegalArgumentException();
            JSObject result = new JSObject(); result.put("text", n.tunnel.client().submit(id)); return result;
        });
    }
    @PluginMethod public void discardPrompt(PluginCall call) {
        perform(call, n -> {
            String id = call.getString("operationId");
            if (id == null) throw new IllegalArgumentException();
            n.tunnel.client().discard(id); return new JSObject();
        });
    }
    @PluginMethod public void status(PluginCall call) { perform(call, this::state); }
    @PluginMethod public void beginSignIn(PluginCall call) {
        clearFeedback();
        perform(call, n -> {
            n.tunnel.disconnect();
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
        clearFeedback();
        // Do not queue behind the blocking device-grant poll.
        try { if (nativeState != null) { nativeState.tunnel.disconnect(); nativeState.owner.cancel(); } call.resolve(); }
        catch (Exception ignored) { call.reject("Sign-in could not be cancelled."); }
    }
    @PluginMethod public void enroll(PluginCall call) {
        clearFeedback();
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
        clearFeedback();
        if (nativeState != null) nativeState.tunnel.disconnect();
        perform(call, n -> {
            MachineStatus current = n.machine.status();
            if (current == null) throw new IllegalStateException();
            n.owner.revokeMachine(n.machine, current.getTenantId()); return state(n);
        });
    }
    @PluginMethod public void connect(PluginCall call) {
        if (nativeState == null) { call.reject("Phone configuration is unavailable."); return; }
        long pendingConnection = nativeState.tunnel.begin();
        call.getData().put("nativeConnectionGeneration", pendingConnection);
        Intent permission = nativeState.tunnel.permissionRequest();
        if (permission == null) startTunnel(call, pendingConnection);
        else startActivityForResult(call, permission, "vpnPermission");
    }
    @ActivityCallback private void vpnPermission(PluginCall call, ActivityResult result) {
        if (call == null) return;
        if (result.getResultCode() != Activity.RESULT_OK) {
            if (nativeState != null) nativeState.tunnel.failed(call.getData().optLong("nativeConnectionGeneration", -1));
            call.reject("VPN permission was not granted."); return;
        }
        startTunnel(call, call.getData().optLong("nativeConnectionGeneration", -1));
    }
    private void startTunnel(PluginCall call, long generation) {
        perform(call, n -> {
            try { n.tunnel.connect(generation); return state(n); }
            catch (Exception failure) { n.tunnel.failed(generation); throw failure; }
        });
    }
    @PluginMethod public void disconnect(PluginCall call) {
        if (nativeState != null) nativeState.tunnel.disconnect();
        perform(call, this::state);
    }
    @Override protected void handleOnDestroy() {
        clearFeedback();
        try { if (nativeState != null) { nativeState.tunnel.disconnect(); nativeState.owner.cancel(); } } catch (Exception ignored) {}
        if (nativeState != null) nativeState.tunnel.close();
        worker.shutdownNow();
    }
}
