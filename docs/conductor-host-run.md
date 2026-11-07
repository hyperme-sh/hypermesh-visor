# Run a coding job from Visor on the test Compose VM

Run the packaged Linux Visor on the test Compose VM. Use an operator-controlled account and a Hypermesh renter key scoped for the trusted test gateway.

1. Select the device and its certified catalog model. For Strix Halo record the larger model's digest, quantization, context length and peak device memory. Check native sell state, a fresh green certification, zero outstanding leases and organization self-use eligibility.
2. In a private directory prepare the endpoints file with `default_class` set to `code` and `recipe_file` pointing to your recipe. Set the frontier model in both files to the exact catalog ID. Set its base URL to `https://chat.test.hyperme.sh/v1` and its auth mode to `passthrough`. Choose the output limit for the job.
3. Arm a timed native pause before running `sudo hypermesh-host sell resume`. Wait for the exact device in the filtered renter host list. Create a device-bound `p2_loaded_model` lease with `self_use=true`, a fresh request ID and the selected catalog ID. Verify the device, active status, zero gross charge and null billing record. Set `lease_id` to the returned lease UUID and `model` to its catalog ID.
4. Start one Visor instance for this lease. Visor allocates a loopback listener, hands its descriptor to the bundled child, and checks readiness before opening the control socket. The parent retains the descriptor to reserve the port throughout the run. Leave `HYPERMESH_CONDUCTOR_ADDR` unset.

   ```sh
   umask 077
   HYPERMESH_RUNTIME_DIR="$PWD/runtime" HYPERMESH_DEFAULT_MODEL="$model" \
     HYPERMESH_CONDUCTOR_CONFIG="$PWD/endpoints.json" \
     HYPERMESH_CONDUCTOR_JOURNAL="$PWD/journal.jsonl" \
     HYPERMESH_CONDUCTOR_LEASE_ID="$lease_id" hypermesh-visor &
   visor_pid=$!
   ```

5. Mint a disposable renter key. POST JSON containing `prompt` and `api_key` to `/prompt` on Visor's `runtime/control.sock`. Pass the JSON via stdin and keep the key out of shell arguments and logs. Omitting `model` selects the configured Visor default. Visor sends it as `X-Conductor-Expected-Model`. Verify the response against deterministic cases, and correlate the Visor audit, Conductor journal model, token counts and recipe hash with the lease and native host job.
6. In cleanup, revoke the key, close the specific lease, pause the host and verify the next heartbeat shows paused with zero active leases. Verify the native stop acknowledgement and workload removal. Stop Visor with `kill "$visor_pid"; wait "$visor_pid"`. Visor owns the child lifecycle. Retrieve this lease's journal rows with `jq -c --arg lease "$lease_id" 'select(.task_id == $lease)' journal.jsonl`. Each prompt gets a separate step ID. The operator supplies the lease ID from checkout. The gateway enforces the actual renter lease.

## J4012 receipt, 2026-11-07

The test Compose VM ran staged Linux Visor and Conductor binaries built from the packaging source pin (Conductor 9917e5f). The AIuditor hop used the local stub. The self-use lease was `8b5afed2-fdfe-5e94-bbea-6c13088fcffc`, gross zero, with a null billing record. J4012 passed start job `2475ea60-7715-43e7-8c8e-d2e6c7072d96`. Visor recorded one forwarded prompt on the Conductor door. Journal task `t54d81ad4cc8eaa30`, step `sf0d1bdd0d6c84b16`, selected frontier model `llama-3.1-8b-q4-nx` and measured 111 input and 28 output tokens in 4,248 ms.

Asked to repair the initial upper bound of a half-open binary search, the model returned a function header and the assignment `lo, hi = 0, len(values)`. The one-line format check rejected the extra header. A separate check inspected the returned assignment's Python AST, completed the supplied search loop, and compared the result with `bisect_left` across 2,016 inputs. All matched on the original response.

Cleanup revoked the renter key and closed the lease. Stop job `a67840b6-920e-449a-b5f4-388d0e52d864` passed. At 18:43:55 UTC the plane heartbeat showed paused, with zero outstanding leases. The host had no running Docker containers. Both VM test processes were stopped.
