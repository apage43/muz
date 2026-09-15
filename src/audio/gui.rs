//! Host-owned X11 windows for plugin editors.
//!
//! CLAP plugins that support X11 embedding paint into a parent window that the
//! host owns, so every open editor gets one plain X11 window here. The serve
//! thread creates it, pumps its lifecycle, and drops it; plugins keep their own
//! event handling and never see this module or the audio callback.

use anyhow::{Context, Result};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
};

/// Lifecycle events the serve loop reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEvent {
    /// The window manager or the user asked to close the window.
    Close,
    /// The window changed size.
    Resized(u32, u32),
}

/// One mapped parent window for one plugin editor.
pub struct EditorWindow {
    connection: x11rb::rust_connection::RustConnection,
    id: xproto::Window,
    delete: xproto::Atom,
    width: u32,
    height: u32,
}

impl EditorWindow {
    /// Map a window of the requested client size that a plugin can embed into.
    pub fn open(title: &str, width: u32, height: u32) -> Result<Self> {
        let (connection, screen) = x11rb::connect(None).context("open the X11 display")?;
        let root = connection.setup().roots[screen].root;
        let depth = connection.setup().roots[screen].root_depth;
        let id = connection.generate_id().context("allocate an X11 window")?;
        connection
            .create_window(
                depth,
                id,
                root,
                0,
                0,
                clamp(width) as u16,
                clamp(height) as u16,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                0,
                &xproto::CreateWindowAux::new()
                    .event_mask(xproto::EventMask::STRUCTURE_NOTIFY | xproto::EventMask::EXPOSURE),
            )
            .context("create the editor window")?;
        let protocols = atom(&connection, b"WM_PROTOCOLS")?;
        let delete = atom(&connection, b"WM_DELETE_WINDOW")?;
        let utf8 = atom(&connection, b"UTF8_STRING")?;
        let net_name = atom(&connection, b"_NET_WM_NAME")?;
        connection.change_property8(
            xproto::PropMode::REPLACE,
            id,
            xproto::AtomEnum::WM_NAME,
            xproto::AtomEnum::STRING,
            title.as_bytes(),
        )?;
        connection.change_property8(
            xproto::PropMode::REPLACE,
            id,
            net_name,
            utf8,
            title.as_bytes(),
        )?;
        connection.change_property8(
            xproto::PropMode::REPLACE,
            id,
            xproto::AtomEnum::WM_CLASS,
            xproto::AtomEnum::STRING,
            b"muz\0muz\0",
        )?;
        connection.change_property32(
            xproto::PropMode::REPLACE,
            id,
            protocols,
            xproto::AtomEnum::ATOM,
            &[delete],
        )?;
        // Identify the window to the compositor like any other host does.
        let normal = atom(&connection, b"_NET_WM_WINDOW_TYPE_NORMAL")?;
        connection.change_property32(
            xproto::PropMode::REPLACE,
            id,
            atom(&connection, b"_NET_WM_WINDOW_TYPE")?,
            xproto::AtomEnum::ATOM,
            &[normal],
        )?;
        connection.change_property32(
            xproto::PropMode::REPLACE,
            id,
            atom(&connection, b"_NET_WM_PID")?,
            xproto::AtomEnum::CARDINAL,
            &[std::process::id()],
        )?;
        connection.map_window(id)?;
        connection.flush()?;
        Ok(Self {
            connection,
            id,
            delete,
            width: clamp(width),
            height: clamp(height),
        })
    }

    /// Identifier that CLAP's `x11` window handle carries.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Resize the window; the plugin's own view is told separately.
    pub fn set_size(&mut self, width: u32, height: u32) -> Result<()> {
        let (width, height) = (clamp(width), clamp(height));
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        self.connection
            .configure_window(
                self.id,
                &xproto::ConfigureWindowAux::new()
                    .width(width)
                    .height(height),
            )
            .context("resize the editor window")?;
        self.connection.flush()?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Drain pending events without blocking.
    pub fn poll(&mut self) -> Result<Vec<WindowEvent>> {
        let mut events = Vec::new();
        while let Some(event) = self
            .connection
            .poll_for_event()
            .context("read editor window events")?
        {
            match event {
                Event::ClientMessage(e)
                    if e.format == 32
                        && e.window == self.id
                        && e.data.as_data32()[0] == self.delete =>
                {
                    events.push(WindowEvent::Close);
                }
                Event::ConfigureNotify(e) if e.window == self.id => {
                    let (width, height) = (u32::from(e.width), u32::from(e.height));
                    if (width, height) != (self.width, self.height) {
                        self.width = width;
                        self.height = height;
                        events.push(WindowEvent::Resized(width, height));
                    }
                }
                Event::DestroyNotify(e) if e.window == self.id => events.push(WindowEvent::Close),
                _ => {}
            }
        }
        Ok(events)
    }
}

impl Drop for EditorWindow {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.id);
        let _ = self.connection.flush();
    }
}

fn atom(connection: &impl Connection, name: &[u8]) -> Result<xproto::Atom> {
    Ok(connection.intern_atom(false, name)?.reply()?.atom)
}

fn clamp(size: u32) -> u32 {
    size.clamp(1, u32::from(u16::MAX))
}
