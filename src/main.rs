#![feature(deque_extras)]

extern crate jack;

extern crate glib;
extern crate gio;
extern crate gio_sys;
extern crate gtk;

extern crate libc;
extern crate epoxy;
extern crate gl;
extern crate shared_library;

use std::sync::{Arc,Mutex};

use std::slice::IterMut;
use std::collections::VecDeque;
use jack::{JackClient,JackPort,JackNframesT};

use gtk::prelude::*;

use std::mem;
use std::ptr;
use std::ffi::CStr;
use gl::types::*;
use shared_library::dynamic_library::DynamicLibrary;

static APP_ID: &'static str = "org.colinkinloch.oscillot";
static APP_PATH: &'static str = "/org/colinkinloch/oscillot";
static JACK_ID: &'static str = "oscillot";

const RESOURCE_BYTES: &'static [u8] = include_bytes!("resources/oscillot.gresource");

struct CallbackData {
  capture: JackPort,
  samples: Arc<Mutex<VecDeque<f32>>>,
  write_cursor: usize,
  samples_outdated: bool,
  length: usize,
  skip: usize,
  record: bool,
  reverse: bool,
  gain: f32
}

fn main() {
  {
    if gtk::init().is_err() {
        println!("Failed to initialize GTK.");
        return;
    }
    let bytes = glib::Bytes::from_static(RESOURCE_BYTES);
    let res = gio::Resource::new_from_data(&bytes)
      .expect("Bad GResource data.");
    gio::resources_register(&res);
  }

  {
    epoxy::load_with(|s| {
      match unsafe { DynamicLibrary::open(None).unwrap().symbol(s) } {
        Ok(v) => v,
        Err(_) => ptr::null(),
      }
    });
    gl::load_with(|s| epoxy::get_proc_addr(s) as *const std::os::raw::c_void);
  }

  let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::empty())
    .expect("Cannot create application.");

  let quit_action = gio::SimpleAction::new("quit", None);
  app.add_action(&quit_action);
  {
    let app = app.clone();
    quit_action.connect_activate(move |_, _| app.quit());
  }

  let client = JackClient::open(JACK_ID, jack::JackNoStartServer);
  let capture = client.register_port(
    &"capture", jack::JACK_DEFAULT_AUDIO_TYPE,jack::JackPortIsInput, 0
  );

  let data = Arc::new(Mutex::new(CallbackData {
    //client: client,
    capture: capture,
    samples: Arc::new(Mutex::new(VecDeque::with_capacity(1024))),
    write_cursor: 0,
    samples_outdated: false,
    length: 1024,
    skip: 1,
    record: true,
    reverse: false,
    gain: 1.0
  }));

  {
    let data = data.lock().unwrap();
    let mut samples = data.samples.lock().unwrap();
    samples.resize(1024, 0.0);
  }

  app.connect_activate(move |app| activate(app, &client, data.clone()));
  app.connect_shutdown(move |app| shutdown(app, &client));

  let args: Vec<String> = std::env::args().collect();
  let args: Vec<&str> = args.iter().map(|arg| arg.as_str()).collect();

  app.run(args.len() as i32, args.as_slice() as &[&str]);
}

fn activate(app: &gtk::Application, client: &JackClient, data: Arc<Mutex<CallbackData>>) {

  let builder = gtk::Builder::new();
  //builder.set_application(app);
  builder.add_from_resource("/org/colinkinloch/oscillot/ui/oscillot.ui")
    .expect("Cannot find ui in resources.");

  let win = builder.get_object::<gtk::ApplicationWindow>("scope-window")
    .expect("Cannot get main window.");
  win.set_application(Some(app));

  let style_context = win.get_style_context()
    .expect("Cannot get style context!");
  let css = gtk::CssProvider::new();
  css.load_from_resource("/org/colinkinloch/oscillot/ui/oscillot.css");

  style_context.add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
  let reverse_action = gio::SimpleAction::new_stateful("reverse", Some(glib::VariantTy::new("b").unwrap()), &glib::Variant::from(true));
  win.add_action(&reverse_action);

  let about_dialog = builder.get_object::<gtk::AboutDialog>("about-dialog")
    .expect("Cannot get about dialog.");
  about_dialog.set_transient_for(Some(&win));
  let about_action = gio::SimpleAction::new("about", None);
  app.add_action(&about_action);
  about_action.connect_activate(move |_, _| about_dialog.show() );

  let gl_area = builder.get_object::<gtk::GLArea>("gl-area")
    .expect("Cannot get gl area!");

  let record_toggle = builder.get_object::<gtk::ToggleButton>("record-toggle")
    .expect("Cannot get record toggle");
  let sample_length_spin = builder.get_object::<gtk::Adjustment>("sample-length")
    .expect("Cannot get sampler spinner");
  let sample_skip_spin = builder.get_object::<gtk::Adjustment>("sample-skip")
    .expect("Cannot get sampler spinner");
  let gain_slider = builder.get_object::<gtk::Adjustment>("gain")
    .expect("Cannot get gain slider");
  let alpha_slider = builder.get_object::<gtk::Adjustment>("alpha")
    .expect("Cannot get gain slider");

  {
    let data = data.clone();
    record_toggle.connect_toggled(move |record_toggle| {
      let mut data = data.lock().unwrap();
      data.record = record_toggle.get_active();
    });
  }
  /*{
    let data = data.lock().unwrap();
    sample_length_spin.set_value(data.length as f64);
    sample_skip_spin.set_value(data.skip as f64);
  }*/
  {
    let data = data.clone();
    sample_skip_spin.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.skip = adj.get_value() as usize;
    });
  }
  {
    let data = data.clone();
    sample_length_spin.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.length = adj.get_value() as usize;
      let mut samples = data.samples.lock().unwrap();
      samples.resize(data.length, 0.0);
    });
  }
  {
    let data = data.clone();
    gain_slider.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.gain = adj.get_value() as f32;
    });
  }
  {
    let gl_area = gl_area.clone();
    alpha_slider.connect_value_changed(move |adj| {
      let v = adj.get_value() as f32;
      gl_area.make_current(); 
      unsafe { gl::ClearColor(0.0, 0.0, 0.0, v) };
      if 1.0 == v {
        gl_area.set_has_alpha(false);
        //css.get_style("");
      } else {
        gl_area.set_has_alpha(true);
      }
    });
  }
  {
    let data = data.clone();
    reverse_action.connect_change_state(move |action, state| {
      let mut data = data.lock().unwrap();
      let state = match state.clone() {
        Some(state) => state,
        None => glib::Variant::from(false)
      };
      action.set_state(&state);
      data.reverse = match state.get::<bool>() {
        Some(state) => state,
        None => false
      };
    });
  }

  let scope_vbuffer: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));
  let scope_varray: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));

  let scope_amp_attr: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));
  let scope_len_uni: Arc<Mutex<GLint>> = Arc::new(Mutex::new(0));

  let scope_program: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));

  {
    let scope_vbuffer = scope_vbuffer.clone();
    let scope_varray = scope_varray.clone();
    let scope_amp_attr = scope_amp_attr.clone();
    let scope_len_uni = scope_len_uni.clone();
    let scope_program = scope_program.clone();

    gl_area.connect_realize(move |gl_area| {
      gl_area.make_current();

      let mut scope_vbuffer = scope_vbuffer.lock().unwrap();
      let mut scope_varray = scope_varray.lock().unwrap();
      let mut scope_amp_attr = scope_amp_attr.lock().unwrap();
      let mut scope_len_uni = scope_len_uni.lock().unwrap();
      let mut scope_program = scope_program.lock().unwrap();

      // TODO Fail good!
      *scope_program = create_program(vec![
        create_shader_for_resource("/org/colinkinloch/oscillot/shaders/scope.glslv", gl::VERTEX_SHADER).unwrap(),
        create_shader_for_resource("/org/colinkinloch/oscillot/shaders/scope.glslf", gl::FRAGMENT_SHADER).unwrap()
      ]).expect("Cannot create program!");

      unsafe {
        gl::ClearColor(0.0, 0.0, 0.0, 1.0);

        *scope_amp_attr = gl::GetAttribLocation(*scope_program, "amplitude\0".as_ptr() as *const GLchar) as GLuint;
        *scope_len_uni = gl::GetUniformLocation(*scope_program, "vert_count\0".as_ptr() as *const GLchar);

        gl::GenVertexArrays(1, &mut *scope_varray);
        gl::BindVertexArray(*scope_varray);

        gl::GenBuffers(1, &mut *scope_vbuffer);
        gl::BindBuffer(epoxy::ARRAY_BUFFER, *scope_vbuffer);

        gl::EnableVertexAttribArray(*scope_amp_attr as GLuint);
        gl::VertexAttribPointer(*scope_amp_attr as GLuint, 1, epoxy::FLOAT,
          epoxy::FALSE as GLboolean, mem::size_of::<f32>() as GLint, ptr::null());
      }
    });
  }

  {
    let scope_vbuffer = scope_vbuffer.clone();
    let scope_varray = scope_varray.clone();
    let scope_amp_attr = scope_amp_attr.clone();
    let scope_len_uni = scope_len_uni.clone();
    let scope_program = scope_program.clone();
    let data = data.clone();
    gl_area.connect_render(move |context, _| {
      let scope_vbuffer = scope_vbuffer.lock().unwrap();
      let scope_varray = scope_varray.lock().unwrap();
      let scope_amp_attr = scope_amp_attr.lock().unwrap();
      let scope_len_uni = scope_len_uni.lock().unwrap();
      let scope_program = scope_program.lock().unwrap();

      let mut data = data.lock().unwrap();
      let samples = data.samples.clone();
      let mut verts = {
        let mut verts = Vec::from((*(samples.lock().unwrap())).clone());
        if data.reverse { verts.reverse() }
        verts
      };

      context.make_current();
      unsafe {
        gl::Clear(epoxy::COLOR_BUFFER_BIT);
        gl::UseProgram(*scope_program);
        gl::BindVertexArray(*scope_varray);
        gl::BindBuffer(epoxy::ARRAY_BUFFER, *scope_vbuffer);
        // TODO More efficient? Mapped memory?
        if data.samples_outdated {
          gl::BufferData(epoxy::ARRAY_BUFFER, (verts.len() * mem::size_of::<f32>()) as GLsizeiptr,
          verts.as_ptr() as *const std::os::raw::c_void, epoxy::DYNAMIC_DRAW);
          gl::Uniform1i(*scope_len_uni, verts.len() as GLint);
        }
        data.samples_outdated = false;
      
        gl::EnableVertexAttribArray(*scope_amp_attr as GLuint);
        gl::DrawArrays(epoxy::LINE_STRIP, 0, verts.len() as GLint);
        gl::DisableVertexAttribArray(*scope_amp_attr as GLuint);
      }
      Inhibit(false)
    });
  }

  win.show_all();

  {
    let data = data.clone();
    let mut data = data.lock().unwrap();
    client.set_process_callback(process, &mut *data);
  }

  {
    let gl_area = gl_area.clone();
    gtk::timeout_add(16, move || {
      gl_area.queue_render();
      glib::Continue(true)
    });
  }

  if !client.activate() {
    println!("Client not active!");
  }
}

fn shutdown(app: &gtk::Application, client: &JackClient) {
  client.close();
}

fn process(frames: JackNframesT, data: *mut CallbackData) -> isize {
  let mut data = unsafe { &mut *data };
  let in_buffer = data.capture.get_vec_buffer::<f32>(frames);
  let mut samples = data.samples.lock().unwrap();
  for (i, v) in in_buffer.iter().enumerate() {
    if data.record && i % data.skip == 0 {
      //samples.push_front(v * data.gain);
      if (data.write_cursor / data.skip) as usize >= samples.len() {
        data.write_cursor = 0;
      }
      *samples.get_mut((data.write_cursor / data.skip) as usize).unwrap() = v * data.gain;
      data.write_cursor = 1 + data.write_cursor;
    }
  }
  //samples.extend(in_buffer);
  data.samples_outdated = true;
  0
}

fn create_shader_for_resource(path: &str, ty: GLenum) -> Result<GLuint, String> {
  let res: Result<glib::Bytes, gio::Error> = unsafe {
    use glib::translate::*;
    let mut error = ptr::null_mut();
    let ret = gio_sys::g_resources_lookup_data(path.to_glib_none().0,
      gio::RESOURCE_LOOKUP_FLAGS_NONE.to_glib(), &mut error);
    if error.is_null() { Ok(from_glib_full(ret)) } else { Err(from_glib_full(error)) }

  };
  let source = match res {
    Ok(v) => v,
    Err(e) => panic!("{}", e)
  };
  //println!("{}", String::from_utf8(Vec::from(&(*source))).unwrap());
  unsafe {
    let shader = gl::CreateShader(ty);
    let psrc = source.as_ptr() as *const GLchar;
    let len = source.len() as GLint;
    gl::ShaderSource(shader, 1, &psrc, &len);
    gl::CompileShader(shader);
    
    let mut status = epoxy::FALSE as GLint;
    gl::GetShaderiv(shader, epoxy::COMPILE_STATUS, &mut status);
    
    if status != (epoxy::TRUE as GLint) {
      let mut len = 0;
      gl::GetShaderiv(shader, epoxy::INFO_LOG_LENGTH, &mut len);
      let mut buf = vec![0i8; len as usize];
      gl::GetShaderInfoLog(shader, len, ptr::null_mut(), buf.as_mut_ptr() as *mut GLchar);
      return Err(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }

    Ok(shader)
  }
}

fn create_program(shaders: Vec<GLuint>) -> Result<GLuint, String> {
  unsafe {
    let program = gl::CreateProgram();
    for shader in shaders {
      gl::AttachShader(program, shader);
    }
    gl::LinkProgram(program);
    
    let mut status = epoxy::FALSE as GLint;
    gl::GetProgramiv(program, epoxy::LINK_STATUS, &mut status);

    // Fail on error
    if status != (epoxy::TRUE as GLint) {
      let mut len: GLint = 0;
      gl::GetProgramiv(program, epoxy::INFO_LOG_LENGTH, &mut len);
      let mut buf = vec![0i8; len as usize];
      gl::GetProgramInfoLog(program, len, ptr::null_mut(), buf.as_mut_ptr() as *mut GLchar);
      return Err(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }

    Ok(program)
  }
}
