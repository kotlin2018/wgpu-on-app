//! copy from wgpu's example

use super::Example;
use app_surface::{AppSurface, SurfaceFrame};
use bytemuck::{Pod, Zeroable};
use rand::{
    distributions::{Distribution, Uniform},
    SeedableRng,
};
use std::{borrow::Cow, mem};
use wgpu::util::DeviceExt;

// number of boid particles to simulate
const NUM_PARTICLES: u32 = 1500;

// number of single-particle calculations (invocations) in each gpu work group
const PARTICLES_PER_GROUP: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SimParams {
    delta_t: f32,
    rule1_distance: f32,
    rule2_distance: f32,
    rule3_distance: f32,
    rule1_scale: f32,
    rule2_scale: f32,
    rule3_scale: f32,
    offset: u32,
}
pub struct Boids {
    particle_bind_groups: Vec<wgpu::BindGroup>,
    dynamic_bind_group: wgpu::BindGroup,
    particle_buffers: Vec<wgpu::Buffer>,
    vertices_buffer: wgpu::Buffer,
    compute_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    work_group_count: u32,
    frame_num: usize,
}

impl Boids {
    pub fn new(app_surface: &AppSurface) -> Self {
        let config = &app_surface.config;
        let device = &app_surface.device;

        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../wgsl_shader/compute.wgsl"
            ))),
        });
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../wgsl_shader/draw.wgsl"
            ))),
        });

        let param_data = SimParams {
            delta_t: 0.04,
            rule1_distance: 0.1,
            rule2_distance: 0.025,
            rule3_distance: 0.025,
            rule1_scale: 0.02,
            rule2_scale: 0.05,
            rule3_scale: 0.005,
            offset: 0u32,
        };

        let sim_param_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 5 * 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for i in 0..5 {
            let mut item = param_data;
            item.offset = 64 * 5 * i;
            app_surface.queue.write_buffer(
                &sim_param_buffer,
                256 * i as wgpu::BufferAddress,
                bytemuck::bytes_of(&item),
            );
        }
        let dynamic_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new((mem::size_of::<SimParams>()) as _),
                },
                count: None,
            }],
            label: None,
        });
        let dynamic_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &&dynamic_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &sim_param_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(256),
                }),
            }],
            label: None,
        });
        // create compute bind layout group and compute pipeline layout
        let bgl_desc = wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((NUM_PARTICLES * 16) as _),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((NUM_PARTICLES * 16) as _),
                    },
                    count: None,
                },
            ],
            label: None,
        };
        let compute_bind_group_layout = device.create_bind_group_layout(&bgl_desc);

        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("compute"),
                bind_group_layouts: &[&compute_bind_group_layout, &dynamic_bgl],
                push_constant_ranges: &[],
            });
        // create render pipeline with empty bind group layout

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("render"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: "main_vs",
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 4 * 4,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 2 * 4,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![2 => Float32x2],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: "main_fs",
                targets: &[Some(config.format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        // create compute pipeline
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Compute pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: "main",
        });

        // buffer for the three 2d triangle vertices of each instance
        let vertex_buffer_data = [-0.01f32, -0.02, 0.01, -0.02, 0.00, 0.02];
        let vertices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::bytes_of(&vertex_buffer_data),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        // buffer for all particles data of type [(posx,posy,velx,vely),...]
        let mut initial_particle_data = vec![0.0f32; (4 * NUM_PARTICLES) as usize];
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let unif = Uniform::new_inclusive(-1.0, 1.0);
        for particle_instance_chunk in initial_particle_data.chunks_mut(4) {
            particle_instance_chunk[0] = unif.sample(&mut rng); // posx
            particle_instance_chunk[1] = unif.sample(&mut rng); // posy
            particle_instance_chunk[2] = unif.sample(&mut rng) * 0.1; // velx
            particle_instance_chunk[3] = unif.sample(&mut rng) * 0.1; // vely
        }

        // creates two buffers of particle data each of size NUM_PARTICLES
        // the two buffers alternate as dst and src for each frame
        let mut particle_buffers = Vec::<wgpu::Buffer>::new();
        let mut particle_bind_groups = Vec::<wgpu::BindGroup>::new();
        for i in 0..2 {
            particle_buffers.push(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("Particle Buffer {}", i)),
                    contents: bytemuck::cast_slice(&initial_particle_data),
                    usage: wgpu::BufferUsages::VERTEX
                        | wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST,
                }),
            );
        }

        // create two bind groups, one for each buffer as the src
        // where the alternate buffer is used as the dst
        for i in 0..2 {
            particle_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &compute_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: particle_buffers[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: particle_buffers[(i + 1) % 2].as_entire_binding(), // bind to opposite buffer
                    },
                ],
                label: None,
            }));
        }

        // calculates number of work groups from PARTICLES_PER_GROUP constant
        let work_group_count =
            ((NUM_PARTICLES as f32) / (PARTICLES_PER_GROUP as f32)).ceil() as u32;

        Self {
            particle_bind_groups,
            dynamic_bind_group,
            particle_buffers,
            vertices_buffer,
            compute_pipeline,
            render_pipeline,
            work_group_count,
            frame_num: 0,
        }
    }
}

impl Example for Boids {
    fn enter_frame(&mut self, app_surface: &AppSurface) {
        let device = &app_surface.device;
        let queue = &app_surface.queue;
        let (frame, view) = app_surface.get_current_frame_view();
        {
            // create render pass descriptor and its color attachments
            let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.8,
                    }),
                    store: true,
                },
            })];
            let render_pass_descriptor = wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
            };

            // get command encoder
            let mut command_encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

            command_encoder.push_debug_group("compute boid movement");
            {
                // compute pass
                let mut cpass = command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    // ty: wgpu::ComputePassType::Concurrent,
                });
                cpass.set_pipeline(&self.compute_pipeline);
                cpass.set_bind_group(0, &self.particle_bind_groups[self.frame_num % 2], &[]);
                // for i in 0..5 {
                //     cpass.set_bind_group(1, &self.dynamic_bind_group, &[256 * i]);
                //     cpass.dispatch_workgroups(5, 1, 1);
                // }
                cpass.set_bind_group(1, &self.dynamic_bind_group, &[256 * 0]);
                cpass.dispatch_workgroups(5, 1, 1);
                // cpass.set_bind_group(1, &self.dynamic_bind_group, &[256 * 1]);
                // cpass.dispatch_workgroups(5, 1, 1);
            }
            {
                let mut cpass = command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    // ty: wgpu::ComputePassType::Concurrent,
                });
                cpass.set_pipeline(&self.compute_pipeline);
                cpass.set_bind_group(0, &self.particle_bind_groups[self.frame_num % 2], &[]);

                for i in 1..5 {
                    cpass.set_bind_group(1, &self.dynamic_bind_group, &[256 * i]);
                    cpass.dispatch_workgroups(5, 1, 1);
                }
            }
            command_encoder.pop_debug_group();

            command_encoder.push_debug_group("render boids");
            {
                // render pass
                let mut rpass = command_encoder.begin_render_pass(&render_pass_descriptor);
                rpass.set_pipeline(&self.render_pipeline);
                // render dst particles
                rpass.set_vertex_buffer(
                    0,
                    self.particle_buffers[(self.frame_num + 1) % 2].slice(..),
                );
                // the three instance-local vertices
                rpass.set_vertex_buffer(1, self.vertices_buffer.slice(..));
                rpass.draw(0..3, 0..NUM_PARTICLES);
            }
            command_encoder.pop_debug_group();

            // done
            queue.submit(Some(command_encoder.finish()));
            queue.submit(None);
        }
        frame.present();
        // update frame count
        self.frame_num += 1;
    }
}
