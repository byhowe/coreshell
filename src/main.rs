use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

const HEIGHT: u32 = 30;

fn main()
{
    // Connect to the wayland server.
    let conn = Connection::connect_to_env().unwrap();
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    // query the globals and bind to required protocols.
    let mut state = State::default();
    let _registry = display.get_registry(&qh, ());
    event_queue.roundtrip(&mut state).unwrap();

    state.exit = false;

    // The main event loop.
    while !state.exit {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

#[derive(Default)]
struct State
{
    // globals
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,

    // created objects
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,

    exit: bool,
}

impl Dispatch<WlRegistry, ()> for State
{
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    )
    {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            // Bind to the globals compositor, shm, layer_shell.
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor: WlCompositor = registry.bind(name, version, qh, ());
                    let surface = compositor.create_surface(qh, ());
                    state.compositor = Some(compositor);
                    state.surface = Some(surface);
                }
                "wl_shm" => {
                    let shm: WlShm = registry.bind(name, version, qh, ());
                    state.shm = Some(shm);
                }
                "zwlr_layer_shell_v1" => {
                    let layer_shell: ZwlrLayerShellV1 = registry.bind(name, version, qh, ());
                    state.layer_shell = Some(layer_shell);
                }
                _ => return,
            }

            // We need both surface and layer_shell to create a layer_surface. Wait for both
            // globals to be available before calling get_layer_surface.
            if let (Some(surface), Some(layer_shell), None) = (
                state.surface.as_ref(),
                state.layer_shell.as_ref(),
                state.layer_surface.as_ref(),
            ) {
                use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
                use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
                let layer_surface = layer_shell.get_layer_surface(
                    surface,
                    None,
                    Layer::Top,
                    String::from("coreshell"),
                    qh,
                    (),
                );
                // Setting 0 to width lets the compositor assign a value and inform us.
                layer_surface.set_size(0, HEIGHT);
                // Anchor top, left, right. This makes the bar appear on the top of the screen
                // stretching from left to right.
                layer_surface.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
                layer_surface.set_exclusive_zone(HEIGHT as _);
                surface.commit();
                state.layer_surface = Some(layer_surface);
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State
{
    fn event(
        _state: &mut Self,
        _layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    )
    {
        // Receive information from the compositor about the width and height of the
        // bar. We need this information to create an appropriately sized buffer
        // in ram to render our pixels to.
        println!("{:?}", event);
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore ZwlrLayerShellV1);
