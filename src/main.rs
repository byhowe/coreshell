use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;

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

    state.exit = true;

    // The main event loop.
    while !state.exit {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

#[derive(Default)]
struct State
{
    // globals
    shm: Option<WlShm>,

    // created objects
    surface: Option<WlSurface>,

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
        println!("{:?}", event);
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            // bind to compositor, shm, layer_shell
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor: WlCompositor = registry.bind(name, version, qh, ());
                    let surface = compositor.create_surface(qh, ());
                    state.surface = Some(surface);
                }
                "wl_shm" => {
                    let shm: WlShm = registry.bind(name, version, qh, ());
                    state.shm = Some(shm);
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
