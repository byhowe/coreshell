use std::os::fd::AsFd;
use std::ptr;
use std::slice;

use rustix::fs::MemfdFlags;
use rustix::fs::ftruncate;
use rustix::fs::memfd_create;
use rustix::mm;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::QueueHandle;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_buffer;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_callback;
use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

const HEIGHT: u32 = 30;
const SPEED: f32 = 0.2;

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

// Generate a scrolling patter.
fn fill_scrolling_stripes(data: &mut [u32], width: usize, stripe_width: usize, offset: usize)
{
    const WHITE: u32 = 0xFFFFFFFF;
    const BLACK: u32 = 0xFF000000;

    let period = stripe_width * 2;
    data.chunks_exact_mut(width).for_each(|row| {
        row.iter_mut().enumerate().for_each(|(i, pixel)| {
            let p = (i + offset) & period;
            *pixel = if p < stripe_width { WHITE } else { BLACK }
        })
    })
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
    buffer: Option<WlBuffer>,

    // pixel data
    width: usize,
    height: usize,
    data: Option<*mut u32>,

    last_frame_time: u32,
    offset: f32,

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
                layer_surface.set_exclusive_zone(HEIGHT as i32);
                surface.commit();
                state.layer_surface = Some(layer_surface);
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State
{
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    )
    {
        // Receive information from the compositor about the width and height of
        // the bar. We need this information to create an appropriately sized
        // buffer in ram to render our pixels to.

        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            println!("Configure Event received");

            layer_surface.ack_configure(serial);

            state.width = width as usize;
            state.height = height as usize;

            let pixels = width * height;
            // Multiply by 4, because each pixel occupies 4 bytes.
            let size = pixels * 4;
            let stride = width * 4;

            // Create a buffer in memory.
            let memfd = memfd_create("coreshell", MemfdFlags::empty()).unwrap();
            // Its size is set by ftruncate.
            ftruncate(&memfd, size as u64).unwrap();
            // Map the file descriptor into memory.
            // FIXME: Do we need to munmap the ptr? I think we would need to munmap if we
            // receive multiple configure events.
            let ptr = unsafe {
                mm::mmap(
                    ptr::null_mut(),
                    size as usize,
                    mm::ProtFlags::READ | mm::ProtFlags::WRITE,
                    mm::MapFlags::SHARED,
                    &memfd,
                    0,
                )
            }
            .unwrap()
            .cast::<u32>();
            state.data = Some(ptr);
            // Treat the raw pointer obtained by mmap as a u32 slice.
            let data = unsafe { slice::from_raw_parts_mut(ptr, pixels as usize) };

            // Create shm pool and buffer.
            // NOTE: When memfd goes out of scope at the end of this function, its drop
            // function will get called and the file descriptor associated with
            // it will be closed. It may happen that the file descriptor closes
            // before it is received by the compositor. In which case, the
            // compositor will fail to open the descriptor (if I understand
            // correctly). However, it seems that file descriptors are sent differently as
            // seen in https://docs.rs/wayland-backend/0.3.10/src/wayland_backend/rs/socket.rs.html#42.
            // The file descriptor received by the compositor will be different than the one
            // we sent. So, I think it is safe for memfd to drop at the end of
            // this scope.
            let shm = state.shm.as_ref().unwrap();
            let shm_pool = shm.create_pool(memfd.as_fd(), size as i32, qh, ());
            let buffer = shm_pool.create_buffer(
                0,
                width as i32,
                height as i32,
                stride as i32,
                wl_shm::Format::Argb8888,
                qh,
                (),
            );
            // NOTE: It is safe to destory the pool after creating a buffer from it.
            // https://wayland.app/protocols/wayland#wl_shm_pool:request:create_buffer
            shm_pool.destroy();

            // Initial paint. Fill with black and white blocks.
            fill_scrolling_stripes(data, width as usize, HEIGHT as usize, 0);

            let surface = state.surface.as_ref().unwrap();
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();

            surface.frame(qh, ());
            state.buffer = Some(buffer);
        }
    }
}

impl Dispatch<WlBuffer, ()> for State
{
    fn event(
        _state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    )
    {
        if let wl_buffer::Event::Release = event {
            // Compositor has done working with the buffer.
        }
    }
}

impl Dispatch<WlCallback, ()> for State
{
    fn event(
        state: &mut Self,
        _callback: &WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    )
    {
        // TODO: Is there a better way to extract the time out of this event?
        if let wl_callback::Event::Done {
            callback_data: time,
        } = event
        {
            let surface = state.surface.as_ref().unwrap();

            // Request another frame.
            surface.frame(qh, ());

            if state.last_frame_time != 0 {
                let elapsed = (time - state.last_frame_time) as f32;
                let period = HEIGHT * 2;
                state.offset += elapsed * SPEED;
                state.offset -= (period * ((state.offset as u32) / period)) as f32;
            }
            let size = state.width * state.height;
            let data = unsafe { slice::from_raw_parts_mut(state.data.unwrap(), size) };
            fill_scrolling_stripes(data, state.width, HEIGHT as usize, state.offset as usize);
            surface.attach(state.buffer.as_ref(), 0, 0);
            surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            surface.commit();
            state.last_frame_time = time;
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore ZwlrLayerShellV1);
