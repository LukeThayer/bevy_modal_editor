//! Primitive shapes spawnable by `EffectAction::SpawnPrimitive`.
//!
//! Moved here from `bevy_modal_editor::scene::primitives` because the effect
//! data model embeds `PrimitiveShape` — the `#[type_path]` pins the original
//! reflected path so saved scenes keep loading. The editor re-exports this
//! type from its old location.

use avian3d::prelude::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Available primitive shapes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect, Default)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub enum PrimitiveShape {
    #[default]
    Cube,
    Sphere,
    Cylinder,
    Capsule,
    Plane,
}

impl PrimitiveShape {
    pub fn display_name(&self) -> &'static str {
        match self {
            PrimitiveShape::Cube => "Cube",
            PrimitiveShape::Sphere => "Sphere",
            PrimitiveShape::Cylinder => "Cylinder",
            PrimitiveShape::Capsule => "Capsule",
            PrimitiveShape::Plane => "Plane",
        }
    }

    /// Create the mesh for this primitive shape (with tangents for normal mapping)
    pub fn create_mesh(&self) -> Mesh {
        let mesh = match self {
            PrimitiveShape::Cube => Mesh::from(Cuboid::new(1.0, 1.0, 1.0)),
            PrimitiveShape::Sphere => Mesh::from(Sphere::new(0.5)),
            PrimitiveShape::Cylinder => Mesh::from(Cylinder::new(0.5, 1.0)),
            PrimitiveShape::Capsule => Mesh::from(Capsule3d::new(0.25, 0.5)),
            PrimitiveShape::Plane => Plane3d::default().mesh().size(2.0, 2.0).build(),
        };
        mesh.with_generated_tangents().expect("primitive mesh should support tangent generation")
    }

    /// Get the default color for this primitive shape
    pub fn default_color(&self) -> Color {
        match self {
            PrimitiveShape::Cube => Color::srgb(0.8, 0.7, 0.6),
            PrimitiveShape::Sphere => Color::srgb(0.6, 0.7, 0.8),
            PrimitiveShape::Cylinder => Color::srgb(0.7, 0.8, 0.6),
            PrimitiveShape::Capsule => Color::srgb(0.8, 0.6, 0.7),
            PrimitiveShape::Plane => Color::srgb(0.6, 0.6, 0.8),
        }
    }

    /// Create a standard material for this primitive shape
    pub fn create_material(&self) -> StandardMaterial {
        StandardMaterial {
            base_color: self.default_color(),
            ..default()
        }
    }

    /// Create the collider for this primitive shape
    pub fn create_collider(&self) -> Collider {
        match self {
            PrimitiveShape::Cube => Collider::cuboid(1.0, 1.0, 1.0),
            PrimitiveShape::Sphere => Collider::sphere(0.5),
            PrimitiveShape::Cylinder => Collider::cylinder(0.5, 1.0),
            PrimitiveShape::Capsule => Collider::capsule(0.25, 0.5),
            PrimitiveShape::Plane => Collider::cuboid(2.0, 0.01, 2.0),
        }
    }
}
