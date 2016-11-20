extern crate jack;

extern crate glib;
extern crate gio_sys;
extern crate gio;
extern crate gdk;
extern crate gtk;

extern crate libc;
extern crate epoxy;
extern crate gl;
extern crate shared_library;

extern crate dft;

use std::sync::{Arc,Mutex};

use jack::{JackClient,JackPort,JackNframesT};

use gdk::prelude::*;
use gtk::prelude::*;

use std::mem;
use std::ptr;
use std::ffi::CStr;
use gl::types::*;
use shared_library::dynamic_library::DynamicLibrary;

static APP_ID: &'static str = "org.colinkinloch.oscillot";
static APP_PATH: &'static str = "/org/colinkinloch/oscillot";

const RESOURCE_BYTES: &'static [u8] = include_bytes!("resources/oscillot.gresource");

const BLEN: usize = 8192;

struct CallbackData {
  capture: JackPort,
  samples: Arc<Mutex<Vec<f32>>>,
  write_cursor: usize,
  samples_outdated: bool,
  length: usize,
  skip: usize,
  record: bool,
  reverse: bool,
  cycle: bool,
  gain: f32,
  rate: JackNframesT
}

fn main() {
  if gtk::init().is_err() {
    println!("hi");
    return;
  }
  let bytes = glib::Bytes::from(&RESOURCE_BYTES);
  let res = gio::Resource::new_from_data(&bytes).unwrap();
  gio::resources_register(&res);

  epoxy::load_with(|s| {
    match unsafe { DynamicLibrary::open(None).unwrap().symbol(s) } {
      Ok(v) => v,
      Err(_) => ptr::null(),
    }
  });
  gl::load_with(epoxy::get_proc_addr);

  let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::empty())
    .expect("Cannot create application.");
  app.set_resource_base_path(Some(APP_PATH));

  let quit_action = gio::SimpleAction::new("quit", None);
  app.add_action(&quit_action);
  {
    let app = app.clone();
    quit_action.connect_activate(move |_, _| app.quit());
  }

  let client = JackClient::open(env!("CARGO_PKG_NAME"), jack::JackNoStartServer);
  let capture = client.register_port(
    &"capture", jack::JACK_DEFAULT_AUDIO_TYPE,jack::JackPortIsInput, 0
  );

  let data = Arc::new(Mutex::new(CallbackData {
    //client: client,
    capture: capture,
    samples: Arc::new(Mutex::new(Vec::with_capacity(BLEN))),
    write_cursor: 0,
    samples_outdated: false,
    length: BLEN,
    skip: 1,
    record: true,
    reverse: false,
    cycle: false,
    gain: 1.0,
    rate: client.sample_rate()
  }));

  {
    let data = data.lock().unwrap();
    let mut samples = data.samples.lock().unwrap();
    samples.resize(BLEN, 0.0);
  }

  app.connect_activate(move |app| activate(app, &client, data.clone()));
  app.connect_shutdown(move |app| shutdown(app, &client));

  let args: Vec<String> = std::env::args().collect();
  let args: Vec<&str> = args.iter().map(|arg| arg.as_str()).collect();

  app.run(args.len() as i32, args.as_slice() as &[&str]);
}

fn activate(app: &gtk::Application, client: &JackClient, data: Arc<Mutex<CallbackData>>) {

  let builder = gtk::Builder::new_from_resource("/org/colinkinloch/oscillot/ui/oscillot.ui");

  let win = builder.get_object::<gtk::ApplicationWindow>("scope-window")
    .expect("Cannot get main window.");
  win.set_application(Some(app));

  let style_context = win.get_style_context()
    .expect("Cannot get style context!");
  let css = gtk::CssProvider::new();
  css.load_from_resource("/org/colinkinloch/oscillot/ui/oscillot.css");
  style_context.add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

  let reverse_action = gio::SimpleAction::new_stateful("reverse",
    Some(glib::VariantTy::new("b").unwrap()), &glib::Variant::from(false));
  let cycle_action = gio::SimpleAction::new_stateful("cycle",
    Some(glib::VariantTy::new("b").unwrap()), &glib::Variant::from(false));
  let fullscreen_action = gio::SimpleAction::new_stateful("fullscreen",
    Some(glib::VariantTy::new("b").unwrap()), &glib::Variant::from(false));
  win.add_action(&reverse_action);
  win.add_action(&cycle_action);
  win.add_action(&fullscreen_action);

  let background_colour = Arc::new(Mutex::new(gdk::RGBA::black()));
  let low_colour = Arc::new(Mutex::new(gdk::RGBA::green()));
  let high_colour = Arc::new(Mutex::new(gdk::RGBA::red()));
  let colour_outdated = Arc::new(Mutex::new(true));

  let connect_colour_button = |colour: &Arc<Mutex<gdk::RGBA>>, button_id: &str| {
    let colour_button = builder.get_object::<gtk::ColorButton>(button_id)
      .expect("Cannot get high colour button");
    let colour = colour.clone();
    colour_button.connect_color_set(move |colour_button|
      *colour.lock().unwrap() = colour_button.get_rgba()
    );
  };
  
  connect_colour_button(&background_colour, "background-colour-button");
  connect_colour_button(&low_colour, "low-colour-button");
  connect_colour_button(&high_colour, "high-colour-button");

  let about_dialog = builder.get_object::<gtk::AboutDialog>("about-dialog")
    .expect("Cannot get about dialog.");
  about_dialog.set_authors(env!("CARGO_PKG_AUTHORS").split(":").collect::<Vec<&str>>().as_slice());
  about_dialog.set_program_name(env!("CARGO_PKG_NAME"));
  about_dialog.set_version(Some(env!("CARGO_PKG_VERSION")));
  about_dialog.set_website(Some(env!("CARGO_PKG_HOMEPAGE")));
  about_dialog.set_comments(Some(env!("CARGO_PKG_DESCRIPTION")));

  let about_action = gio::SimpleAction::new("about", None);
  app.add_action(&about_action);
  about_action.connect_activate(move |_, _| about_dialog.show() );

  connect_ui_signals(&builder, &data);

  let gl_area = builder.get_object::<gtk::GLArea>("gl-area")
    .expect("Cannot get gl area!");

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
  {
    let data = data.clone();
    let cycle_button = builder.get_object::<gtk::ToggleButton>("cycle-toggle")
      .expect("Cannot get cycle button!");
    cycle_action.connect_activate(move |_action, _state| {
      let cycle = cycle_button.get_active();
      let mut data = data.lock().unwrap();
      data.cycle = cycle;
    });
  }
  {
    let fullscreen_button = builder.get_object::<gtk::ToggleButton>("fullscreen-toggle")
      .expect("Cannot get fullscreen button!");
    let win = win.clone();
    fullscreen_action.connect_activate(move |_action, _state| {
      let fullscreen = fullscreen_button.get_active();
      if fullscreen {
        win.fullscreen();
      } else {
        win.unfullscreen();
      }
    });
  }

  let scope_vbuffer: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));
  let scope_varray: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));

  let scope_amp_attr: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));
  let scope_len_uni: Arc<Mutex<GLint>> = Arc::new(Mutex::new(0));

  let scope_low_col_uni: Arc<Mutex<GLint>> = Arc::new(Mutex::new(0));
  let scope_high_col_uni: Arc<Mutex<GLint>> = Arc::new(Mutex::new(0));

  let scope_program: Arc<Mutex<GLuint>> = Arc::new(Mutex::new(0));

  {
    let scope_vbuffer = scope_vbuffer.clone();
    let scope_varray = scope_varray.clone();
    let scope_amp_attr = scope_amp_attr.clone();
    let scope_len_uni = scope_len_uni.clone();
    let scope_low_col_uni = scope_low_col_uni.clone();
    let scope_high_col_uni = scope_high_col_uni.clone();
    let scope_program = scope_program.clone();

    gl_area.connect_realize(move |gl_area| {
      gl_area.make_current();

      let mut scope_vbuffer = scope_vbuffer.lock().unwrap();
      let mut scope_varray = scope_varray.lock().unwrap();
      let mut scope_amp_attr = scope_amp_attr.lock().unwrap();
      let mut scope_len_uni = scope_len_uni.lock().unwrap();
      let mut scope_low_col_uni = scope_low_col_uni.lock().unwrap();
      let mut scope_high_col_uni = scope_high_col_uni.lock().unwrap();
      let mut scope_program = scope_program.lock().unwrap();

      // TODO Fail good!
      *scope_program = create_program(vec![
        create_shader_for_resource("/org/colinkinloch/oscillot/shaders/scope.glslv", gl::VERTEX_SHADER)
          .expect("Cannot create Vertex Shader"),
        create_shader_for_resource("/org/colinkinloch/oscillot/shaders/scope.glslf", gl::FRAGMENT_SHADER)
          .expect("Cannot create Fragment Shader")
      ]).expect("Cannot create program!");

      unsafe {
        gl::ClearColor(0.0, 0.0, 0.0, 1.0);

        *scope_amp_attr = gl::GetAttribLocation(*scope_program, "amplitude\0".as_ptr() as *const GLchar) as GLuint;
        *scope_len_uni = gl::GetUniformLocation(*scope_program, "vert_count\0".as_ptr() as *const GLchar);
        *scope_low_col_uni = gl::GetUniformLocation(*scope_program, "low_colour\0".as_ptr() as *const GLchar);
        *scope_high_col_uni = gl::GetUniformLocation(*scope_program, "high_colour\0".as_ptr() as *const GLchar);

        gl::GenVertexArrays(1, &mut *scope_varray);
        gl::BindVertexArray(*scope_varray);

        gl::GenBuffers(1, &mut *scope_vbuffer);
        gl::BindBuffer(epoxy::ARRAY_BUFFER, *scope_vbuffer);

        gl::EnableVertexAttribArray(*scope_amp_attr as GLuint);
        gl::VertexAttribPointer(*scope_amp_attr as GLuint, 1, epoxy::FLOAT,
          epoxy::FALSE as GLboolean, mem::size_of::<f32>() as GLint, ptr::null());

        gl::UseProgram(*scope_program);
      }
    });
  }

  {
    let scope_len_uni = scope_len_uni.clone();
    let data = data.clone();
    let background_colour = background_colour.clone();
    gl_area.connect_render(move |gl_area, _| {
      let scope_len_uni = scope_len_uni.lock().unwrap();

      let mut data = data.lock().unwrap();
      let samples = data.samples.clone();
      let verts = {
        let mut verts = Vec::new();//Vec::from((*(samples.lock().unwrap())).clone());
        if data.cycle {
          verts = samples.lock().unwrap().clone();
        } else {
          let v = samples.lock().unwrap().clone();
          if (data.write_cursor / data.skip) as usize >= v.len() {
            data.write_cursor = 0;
          }
          let (v1, v2) = v.split_at((data.write_cursor / data.skip) as usize);
          verts.extend_from_slice(v2);
          verts.extend_from_slice(v1);
        }
        if data.reverse { verts.reverse() };
        let freqs = {
          use dft::*;
          use dft::Operation::*;
          let mut input = {
            let l: usize = (10.0 as f32).exp2() as usize;
            let mut input = Vec::with_capacity(l);
            input.resize(l, 0.0);
            for (i, s) in input.iter_mut().zip(samples.lock().unwrap().iter().cycle()) {
              *i = *s as f64;
              //println!("{}", s);
            }
            //.map(|&v| v as f64).collect::<Vec<_>>();
            input
          };
          let plan = Plan::new(Forward, input.len());
          transform(&mut input, &plan);
          let mut out = unpack(&input).iter().map(|&v| v.norm().log10() as f32).collect::<Vec<_>>();
          out.split_off((input.len() / 2));
          out
        };
        
        let mut top_freq = 0.0;
        let mut top_freq_v = 0.0;
        
        for (i, v) in freqs.iter().enumerate() {
          if v >= &top_freq_v {
            top_freq = i as f32 * data.rate as f32 / freqs.len() as f32;
            top_freq_v = *v as f32;
          }
        }
        /*println!("top: {} Hz at {}", top_freq, top_freq_v);
        if(top_freq_v != 0.0) {
          for v in freqs.iter_mut() {
            *v = *v / top_freq_v;
          }
        }*/
        
        //verts
        freqs
      };

      gl_area.make_current();
      if *colour_outdated.lock().unwrap() {
        let lc = low_colour.lock().unwrap();
        let hc = high_colour.lock().unwrap();
        let bc = background_colour.lock().unwrap();
        // TODO Only update on change
        if 1.0 == lc.alpha && 1.0 == hc.alpha && 1.0 == bc.alpha {
          gl_area.set_has_alpha(false);
          style_context.remove_class("transparent");
        } else {
          gl_area.set_has_alpha(true);
          style_context.add_class("transparent");
        }
        let scope_low_col_uni = scope_low_col_uni.lock().unwrap();
        let scope_high_col_uni = scope_high_col_uni.lock().unwrap();
        unsafe {
          gl::Uniform4f(*scope_low_col_uni, lc.red as f32, lc.green as f32, lc.blue as f32, lc.alpha as f32);
          gl::Uniform4f(*scope_high_col_uni, hc.red as f32, hc.green as f32, hc.blue as f32, hc.alpha as f32);
          gl::ClearColor(bc.red as f32, bc.green as f32, bc.blue as f32, bc.alpha as f32)
        };
      }
      unsafe {
        gl::Clear(epoxy::COLOR_BUFFER_BIT);
        // TODO More efficient? Mapped memory?
        if data.samples_outdated {
          gl::BufferData(epoxy::ARRAY_BUFFER, (verts.len() * mem::size_of::<f32>()) as GLsizeiptr,
          verts.as_ptr() as *const GLvoid, epoxy::STREAM_DRAW);
          gl::Uniform1i(*scope_len_uni, verts.len() as GLint);
          data.samples_outdated = false;
        }
      
        gl::DrawArrays(epoxy::LINE_STRIP, 0, verts.len() as GLint);
      }
      Inhibit(false)
    });
  }

        println!("Yolo");
  win.show_all();
  //win.show();
        println!("Yolu");

  {
    let data = data.clone();
    let mut data = data.lock().unwrap();
    client.set_process_callback(process, &mut *data);
  }

  {
    let gl_area = gl_area.clone();
    gtk::timeout_add(8, move || {
      gl_area.queue_render();
      glib::Continue(true)
    });
  }

  if !client.activate() {
    println!("Client not active!");
  }
}

fn shutdown(_app: &gtk::Application, client: &JackClient) {
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
      match samples.get_mut((data.write_cursor / data.skip) as usize) {
        Some(sample) => *sample = v * data.gain,
        None => {}
      }
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
      Err(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    } else {
      Ok(shader)
    }
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

fn connect_ui_signals(builder: &gtk::Builder, data: &std::sync::Arc<std::sync::Mutex<CallbackData>>) {
  {
    let data = data.clone();
    let record_toggle = builder.get_object::<gtk::ToggleButton>("record-toggle")
      .expect("Cannot get record toggle");
    record_toggle.connect_toggled(move |record_toggle| {
      let mut data = data.lock().unwrap();
      data.record = record_toggle.get_active();
    });
  }
  {
    let data = data.clone();
    let sample_skip_spin = builder.get_object::<gtk::Adjustment>("sample-skip")
      .expect("Cannot get sampler spinner");
    sample_skip_spin.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.skip = adj.get_value() as usize;
    });
  }
  {
    let data = data.clone();
    let sample_length_spin = builder.get_object::<gtk::SpinButton>("sample-length-spin")
      .expect("Cannot get sampler spinner");
    sample_length_spin.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.length = adj.get_value().exp2() as usize;
      let mut samples = data.samples.lock().unwrap();
      samples.resize(data.length, 0.0);
    });
  }
  {
    let data = data.clone();
    let gain_slider = builder.get_object::<gtk::Adjustment>("gain")
      .expect("Cannot get gain slider");
    gain_slider.connect_value_changed(move |adj| {
      let mut data = data.lock().unwrap();
      data.gain = adj.get_value() as f32;
    });
  }
}
