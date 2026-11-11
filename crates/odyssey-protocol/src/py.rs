//! Python conversions for the payload types pyo3 cannot derive: a `#[pyclass]`
//! complex enum needs every variant field convertible, and the orphan rule
//! blocks that for `heapless` types and `Result`.

use pyo3::types::{PyAnyMethods as _, PyList, PyString, PyStringMethods as _};
use pyo3::{Borrowed, Bound, FromPyObject, IntoPyObject, PyAny, PyErr, PyResult, Python};

use crate::{DeviceName, GeneralConfig, List};

impl<'py> IntoPyObject<'py> for DeviceName {
    type Target = PyString;
    type Output = Bound<'py, PyString>;
    type Error = core::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(PyString::new(py, self.as_str()))
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for DeviceName {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let s = obj.cast::<PyString>()?;
        let s = s.to_cow()?;
        if s.len() > 32 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "device name must be at most 32 bytes",
            ));
        }
        Ok(Self::from_str_lossy(&s))
    }
}

impl<'py, T, const N: usize> IntoPyObject<'py> for List<T, N>
where
    T: IntoPyObject<'py>,
{
    type Target = PyList;
    type Output = Bound<'py, PyList>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        PyList::new(py, self.0)
    }
}

impl<'a, 'py, T, const N: usize> FromPyObject<'a, 'py> for List<T, N>
where
    T: pyo3::prelude::FromPyObjectOwned<'py>,
{
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let mut out = heapless::Vec::new();
        for item in obj.try_iter()? {
            let item = item?;
            let item = item
                .extract::<T>()
                .map_err(|e| -> PyErr { Into::<PyErr>::into(e) })?;
            out.push(item).map_err(|_rejected_item| {
                pyo3::exceptions::PyValueError::new_err("too many items for this message")
            })?;
        }
        Ok(Self(out))
    }
}

/// Python sees the flat wire form. The enum's own payloads are foreign types.
impl<'py> IntoPyObject<'py> for GeneralConfig {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        use pyo3::IntoPyObjectExt as _;
        crate::wire::GeneralConfig::from(self).into_bound_py_any(py)
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for GeneralConfig {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        Ok(obj.extract::<crate::wire::GeneralConfig>()?.into())
    }
}

/// Completion uses `(id, reply, error)` because Python has no Rust `Result`.
impl<'py> IntoPyObject<'py> for crate::accessory::Completion {
    type Target = pyo3::types::PyTuple;
    type Output = Bound<'py, Self::Target>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        let (reply, error) = match self.result {
            Ok(reply) => (Some(reply), None),
            Err(error) => (None, Some(error)),
        };
        (self.id, reply, error).into_pyobject(py)
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for crate::accessory::Completion {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let (id, reply, error): (
            u32,
            Option<crate::accessory::Reply>,
            Option<crate::accessory::Error>,
        ) = obj.extract()?;
        let result = match (reply, error) {
            (Some(reply), None) => Ok(reply),
            (None, Some(error)) => Err(error),
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "completion needs exactly one reply or error",
                ));
            }
        };
        Ok(Self { id, result })
    }
}
