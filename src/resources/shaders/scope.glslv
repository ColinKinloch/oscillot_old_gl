#version 300 es
precision mediump float;

uniform int vert_count;
uniform vec4 low_colour;
uniform vec4 high_colour;

in float amplitude;
//in int gl_VertexID;

out vec4 v_colour;

void main() {
  float mag = abs(amplitude);
  v_colour = high_colour * mag + low_colour * (1.0 - mag);//vec4(mag, 1.0 - mag, 0.0, 1.0);
  float p = float(gl_VertexID) / float(vert_count - 1);
  gl_Position = vec4(p * 2.0 - 1.0, amplitude, 0.0, 1.0);
}
