//! Python conversions for the payload types pyo3 cannot derive: a `#[pyclass]`
//! complex enum needs every variant field convertible, and the orphan rule
//! blocks that for `heapless` types and `Result`.

use pyo3::types::{PyAnyMethods, PyList, PyString, PyStringMethods};
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
        Ok(DeviceName::from_str_lossy(&s))
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
            out.push(item).map_err(|_| {
                pyo3::exceptions::PyValueError::new_err("too many items for this message")
            })?;
        }
        Ok(List(out))
    }
}

/// Python sees the flat wire form; the enum's own payloads are foreign types.
impl<'py> IntoPyObject<'py> for GeneralConfig {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        use pyo3::IntoPyObjectExt;
        crate::wire::GeneralConfig::from(self).into_bound_py_any(py)
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for GeneralConfig {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        Ok(obj.extract::<crate::wire::GeneralConfig>()?.into())
    }
}
