# Run a coding job from Visor on the test Compose VM

Use an operator-controlled VM account for both processes. The packaged Linux Conductor binary is at `/usr/lib/hypermesh-visor/conductor`. Visor sends the renter bearer to the configured listener, so confirm the listener belongs to that binary before submitting a prompt. Use a Hypermesh renter key only with a trusted Hypermesh gateway configured for passthrough.

1. Select the device and its certified model catalog ID. For Strix Halo, record the larger model digest, quantization and context length. Check native sell status, a fresh green certification and zero outstanding leases. Confirm checkout supports organization self-use.
2. Create a private run directory. Set the endpoints file's `listen` to an unused numeric loopback address, `default_class` to `code`, and `recipe_file` to your recipe. Set the frontier model to the exact catalog ID in both files. Use `https://chat.test.hyperme.sh/v1` with `auth: passthrough` on the selected tier. Configure the recipe's output limit for the job.
3. Start Conductor and Visor as the VM user. Substitute the model ID and address from step 2. Keep the process IDs for cleanup. Check Conductor owns the listening port and Visor created the private control socket.

   ```sh
   umask 077
   /usr/lib/hypermesh-visor/conductor serve --config endpoints.json --journal journal.jsonl &
   conductor_pid=$!
   HYPERMESH_RUNTIME_DIR="$PWD/runtime" HYPERMESH_DEFAULT_MODEL="$model" \
     HYPERMESH_CONDUCTOR_ADDR="$addr" hypermesh-visor &
   visor_pid=$!
   ```

4. Arm a timed native pause before running `sudo hypermesh-host sell resume`. Wait for the exact device in the renter host list filtered by catalog ID. Create a device-bound `p2_loaded_model` lease with `self_use=true`, a fresh request ID and the selected catalog ID. Verify the device, zero gross charge, null billing record and active status.
5. Mint a disposable renter key. POST JSON containing `prompt` and `api_key` to `/prompt` on Visor's `runtime/control.sock`. Pass the JSON via stdin and keep the key out of shell arguments and logs. Omitting `model` selects the configured Visor default. Visor sends it as `X-Conductor-Expected-Model`. Verify the response against deterministic cases, and correlate the Visor audit, Conductor journal model, token counts and recipe hash with the lease and native host job.
6. In cleanup, revoke the key, close the specific lease, pause the host and verify the next heartbeat shows paused with zero active leases. Verify the native stop acknowledgement and workload removal. Stop both VM processes and retain the credential-free receipt.

## J4012 receipt, 2026-11-07

The test Compose VM ran staged Linux Visor and Conductor binaries built from the packaging source pin (Conductor 9917e5f). The AIuditor hop used the local stub. The self-use lease was `8b5afed2-fdfe-5e94-bbea-6c13088fcffc`, gross zero, with a null billing record. J4012 passed start job `2475ea60-7715-43e7-8c8e-d2e6c7072d96`. Visor recorded one forwarded prompt on the Conductor door. Journal task `t54d81ad4cc8eaa30`, step `sf0d1bdd0d6c84b16`, selected frontier model `llama-3.1-8b-q4-nx` and measured 111 input and 28 output tokens in 4,248 ms.

Asked to repair the initial upper bound of a half-open binary search, the model returned a function header and the assignment `lo, hi = 0, len(values)`. The one-line format check rejected the extra header. A separate check inspected the returned assignment's Python AST, completed the supplied search loop, and compared the result with `bisect_left` across 2,016 inputs. All matched on the original response.

Cleanup revoked the renter key and closed the lease. Stop job `a67840b6-920e-449a-b5f4-388d0e52d864` passed. At 18:43:55 UTC the plane heartbeat showed paused, with zero outstanding leases. The host had no running Docker containers. Both VM test processes were stopped.

For the next run, automate both process lifetimes and listener ownership, keep the lease and journal IDs together, and use a verifier that distinguishes formatting from correctness.
