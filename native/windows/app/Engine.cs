using System.Reflection;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace Hypermesh.Visor;

internal static class Engine
{
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate int Callback(nint request, nuint length, out nint output, out nuint outputLength);
    [DllImport("hypermesh_visor", CallingConvention = CallingConvention.Cdecl)]
    private static extern int hypermesh_native_start(Callback callback);
    [DllImport("hypermesh_visor", CallingConvention = CallingConvention.Cdecl)]
    private static extern int hypermesh_native_control(byte[] request, nuint length, out nint output, out nuint outputLength);
    [DllImport("hypermesh_visor", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint hypermesh_native_alloc(nuint length);
    [DllImport("hypermesh_visor", CallingConvention = CallingConvention.Cdecl)]
    private static extern void hypermesh_native_free(nint pointer);
    private static readonly SemaphoreSlim Calls = new(1);
    private static Callback? callback;

    static Engine()
    {
        NativeLibrary.SetDllImportResolver(typeof(Engine).Assembly, (name, assembly, search) =>
            name == "hypermesh_visor" ? NativeLibrary.Load(Path.Combine(AppContext.BaseDirectory, "hypermesh_visor.dll")) : 0);
    }

    internal static bool Start(Func<JsonElement, object> desktop)
    {
        callback = (nint request, nuint length, out nint output, out nuint outputLength) =>
        {
            output = 0; outputLength = 0;
            if (request == 0 || length == 0 || length > 1024 * 1024) return 1;
            byte[] bytes = new byte[(int)length];
            Marshal.Copy(request, bytes, 0, bytes.Length);
            byte[] response;
            try
            {
                using JsonDocument input = JsonDocument.Parse(bytes, new JsonDocumentOptions { MaxDepth = 32 });
                response = JsonSerializer.SerializeToUtf8Bytes(new { result = desktop(input.RootElement) });
            }
            catch { response = JsonSerializer.SerializeToUtf8Bytes(new { error = "Native operation refused or unavailable" }); }
            if (response.Length > 48 * 1024 * 1024) return 1;
            output = hypermesh_native_alloc((nuint)response.Length);
            if (output == 0) return 1;
            Marshal.Copy(response, 0, output, response.Length);
            outputLength = (nuint)response.Length;
            return 0;
        };
        return hypermesh_native_start(callback) == 0;
    }

    internal static async Task<JsonElement?> Control(string action, string? session = null, ulong? epoch = null)
    {
        await Calls.WaitAsync();
        try
        {
            return await Task.Run(() =>
            {
                byte[] request = JsonSerializer.SerializeToUtf8Bytes(new { action, session, epoch });
                int status = hypermesh_native_control(request, (nuint)request.Length, out nint output, out nuint length);
                try
                {
                    if (status != 0 || output == 0 || length == 0 || length > 256 * 1024) return (JsonElement?)null;
                    byte[] bytes = new byte[(int)length];
                    Marshal.Copy(output, bytes, 0, bytes.Length);
                    using JsonDocument response = JsonDocument.Parse(bytes, new JsonDocumentOptions { MaxDepth = 32 });
                    return response.RootElement.Clone();
                }
                finally { hypermesh_native_free(output); }
            });
        }
        catch { return null; }
        finally { Calls.Release(); }
    }
}
