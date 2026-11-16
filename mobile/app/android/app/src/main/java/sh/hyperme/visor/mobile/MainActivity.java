package sh.hyperme.visor.mobile;

import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {
    @Override public void onCreate(android.os.Bundle state) {
        registerPlugin(VisorPlugin.class);
        super.onCreate(state);
    }
}
