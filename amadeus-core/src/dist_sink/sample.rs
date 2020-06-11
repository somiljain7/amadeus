#![allow(clippy::type_complexity)]

use futures::{ready, Stream};
use pin_project::pin_project;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::{
	hash::Hash, marker::PhantomData, pin::Pin, task::{Context, Poll}
};
use streaming_algorithms::{SampleUnstable as SASampleUnstable, Top, Zeroable};

use super::{
	DistributedPipe, DistributedSink, Factory, Reducer, ReducerAsync, ReducerProcessSend, ReducerSend, SumReduceFactory, SumReducer
};
use crate::pool::ProcessSend;

#[must_use]
pub struct SampleUnstable<I> {
	i: I,
	samples: usize,
}
impl<I> SampleUnstable<I> {
	pub(crate) fn new(i: I, samples: usize) -> Self {
		Self { i, samples }
	}
}

impl<I: DistributedPipe<Source>, Source> DistributedSink<I, Source, SASampleUnstable<I::Item>>
	for SampleUnstable<I>
where
	I::Item: ProcessSend,
{
	type ReduceAFactory = SampleUnstableReduceFactory<I::Item>;
	type ReduceBFactory = SumReduceFactory<SASampleUnstable<I::Item>, SASampleUnstable<I::Item>>;
	type ReduceA = SampleUnstableReducer<I::Item>;
	type ReduceB = SumReducer<SASampleUnstable<I::Item>, SASampleUnstable<I::Item>>;
	type ReduceC = SumReducer<SASampleUnstable<I::Item>, SASampleUnstable<I::Item>>;

	fn reducers(self) -> (I, Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		(
			self.i,
			SampleUnstableReduceFactory(self.samples, PhantomData),
			SumReduceFactory::new(), // TODO: pass SASampleUnstable::new(self.samples) ?
			SumReducer::new(SASampleUnstable::new(self.samples)),
		)
	}
}

#[derive(Serialize, Deserialize)]
#[serde(
	bound(serialize = "A: Serialize"),
	bound(deserialize = "A: Deserialize<'de>")
)]
pub struct SampleUnstableReduceFactory<A>(usize, PhantomData<fn(A)>);
impl<A> Factory for SampleUnstableReduceFactory<A> {
	type Item = SampleUnstableReducer<A>;
	fn make(&self) -> Self::Item {
		SampleUnstableReducer(Some(SASampleUnstable::new(self.0)))
	}
}
impl<A> Clone for SampleUnstableReduceFactory<A> {
	fn clone(&self) -> Self {
		Self(self.0, PhantomData)
	}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
pub struct SampleUnstableReducer<A>(Option<SASampleUnstable<A>>);

impl<A> Reducer for SampleUnstableReducer<A> {
	type Item = A;
	type Output = SASampleUnstable<A>;
	type Async = Self;

	fn into_async(self) -> Self::Async {
		self
	}
}
impl<A> ReducerAsync for SampleUnstableReducer<A> {
	type Item = A;
	type Output = SASampleUnstable<A>;

	#[inline(always)]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context,
		mut stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let self_ = self.project();
		let self_0 = self_.0.as_mut().unwrap();
		while let Some(item) = ready!(stream.as_mut().poll_next(cx)) {
			self_0.push(item, &mut thread_rng());
		}
		Poll::Ready(())
	}
	fn poll_output(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
		Poll::Ready(self.project().0.take().unwrap())
	}
}
impl<A> ReducerProcessSend for SampleUnstableReducer<A>
where
	A: ProcessSend,
{
	type Output = SASampleUnstable<A>;
}
impl<A> ReducerSend for SampleUnstableReducer<A>
where
	A: Send + 'static,
{
	type Output = SASampleUnstable<A>;
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NonzeroReduceFactory<RF>(RF)
where
	RF: Factory;
impl<RF, B> Factory for NonzeroReduceFactory<RF>
where
	RF: Factory,
	RF::Item: Reducer<Output = Zeroable<B>>,
{
	type Item = NonzeroReducer<RF::Item>;

	fn make(&self) -> Self::Item {
		NonzeroReducer(self.0.make())
	}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
pub struct NonzeroReducer<R>(#[pin] R);

impl<R, B> Reducer for NonzeroReducer<R>
where
	R: Reducer<Output = Zeroable<B>>,
{
	type Item = R::Item;
	type Output = B;
	type Async = NonzeroReducer<R::Async>;

	fn into_async(self) -> Self::Async {
		NonzeroReducer(self.0.into_async())
	}
}
impl<R, B> ReducerAsync for NonzeroReducer<R>
where
	R: ReducerAsync<Output = Zeroable<B>>,
{
	type Item = R::Item;
	type Output = B;

	#[inline(always)]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context, stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		self.project().0.poll_forward(cx, stream)
	}
	fn poll_output(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		self.project()
			.0
			.poll_output(cx)
			.map(|item| item.nonzero().unwrap())
	}
}
impl<R, B> ReducerProcessSend for NonzeroReducer<R>
where
	R: Reducer<Output = Zeroable<B>> + ProcessSend,
	B: ProcessSend,
{
	type Output = B;
}
impl<R, B> ReducerSend for NonzeroReducer<R>
where
	R: Reducer<Output = Zeroable<B>> + Send + 'static,
	B: Send + 'static,
{
	type Output = B;
}

#[must_use]
pub struct MostFrequent<I> {
	i: I,
	n: usize,
	probability: f64,
	tolerance: f64,
}
impl<I> MostFrequent<I> {
	pub(crate) fn new(i: I, n: usize, probability: f64, tolerance: f64) -> Self {
		Self {
			i,
			n,
			probability,
			tolerance,
		}
	}
}

impl<I: DistributedPipe<Source>, Source> DistributedSink<I, Source, Top<I::Item, usize>>
	for MostFrequent<I>
where
	I::Item: Clone + Hash + Eq + ProcessSend,
{
	type ReduceAFactory = MostFrequentReduceFactory<I::Item>;
	type ReduceBFactory =
		NonzeroReduceFactory<SumReduceFactory<Top<I::Item, usize>, Zeroable<Top<I::Item, usize>>>>;
	type ReduceA = MostFrequentReducer<I::Item>;
	type ReduceB = NonzeroReducer<SumReducer<Top<I::Item, usize>, Zeroable<Top<I::Item, usize>>>>;
	type ReduceC = NonzeroReducer<SumReducer<Top<I::Item, usize>, Zeroable<Top<I::Item, usize>>>>;

	fn reducers(self) -> (I, Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		(
			self.i,
			MostFrequentReduceFactory(self.n, self.probability, self.tolerance, PhantomData),
			NonzeroReduceFactory(SumReduceFactory::new()),
			NonzeroReducer(SumReducer::new(Zeroable::Nonzero(Top::new(
				self.n,
				self.probability,
				self.tolerance,
				(),
			)))),
		)
	}
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct MostFrequentReduceFactory<A>(usize, f64, f64, PhantomData<fn(A)>);
impl<A> Factory for MostFrequentReduceFactory<A>
where
	A: Clone + Hash + Eq,
{
	type Item = MostFrequentReducer<A>;
	fn make(&self) -> Self::Item {
		MostFrequentReducer(Some(Top::new(self.0, self.1, self.2, ())))
	}
}
impl<A> Clone for MostFrequentReduceFactory<A> {
	fn clone(&self) -> Self {
		Self(self.0, self.1, self.2, PhantomData)
	}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
#[serde(bound(
	serialize = "A: Hash + Eq + Serialize",
	deserialize = "A: Hash + Eq + Deserialize<'de>"
))]
pub struct MostFrequentReducer<A>(Option<Top<A, usize>>);

impl<A> Reducer for MostFrequentReducer<A>
where
	A: Clone + Hash + Eq,
{
	type Item = A;
	type Output = Top<A, usize>;
	type Async = Self;

	fn into_async(self) -> Self::Async {
		self
	}
}
impl<A> ReducerAsync for MostFrequentReducer<A>
where
	A: Clone + Hash + Eq,
{
	type Item = A;
	type Output = Top<A, usize>;

	#[inline(always)]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context,
		mut stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let self_ = self.project();
		let self_0 = self_.0.as_mut().unwrap();
		while let Some(item) = ready!(stream.as_mut().poll_next(cx)) {
			self_0.push(item, &1);
		}
		Poll::Ready(())
	}
	fn poll_output(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
		Poll::Ready(self.project().0.take().unwrap())
	}
}
impl<A> ReducerProcessSend for MostFrequentReducer<A>
where
	A: Clone + Hash + Eq + ProcessSend,
{
	type Output = Top<A, usize>;
}
impl<A> ReducerSend for MostFrequentReducer<A>
where
	A: Clone + Hash + Eq + Send + 'static,
{
	type Output = Top<A, usize>;
}

#[must_use]
pub struct MostDistinct<I> {
	i: I,
	n: usize,
	probability: f64,
	tolerance: f64,
	error_rate: f64,
}
impl<I> MostDistinct<I> {
	pub(crate) fn new(i: I, n: usize, probability: f64, tolerance: f64, error_rate: f64) -> Self {
		Self {
			i,
			n,
			probability,
			tolerance,
			error_rate,
		}
	}
}

impl<I: DistributedPipe<Source, Item = (A, B)>, Source, A, B>
	DistributedSink<I, Source, Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>>
	for MostDistinct<I>
where
	A: Clone + Hash + Eq + ProcessSend,
	B: Hash + 'static,
{
	type ReduceAFactory = MostDistinctReduceFactory<A, B>;
	type ReduceBFactory = NonzeroReduceFactory<
		SumReduceFactory<
			Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>,
			Zeroable<Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>>,
		>,
	>;
	type ReduceA = MostDistinctReducer<A, B>;
	type ReduceB = NonzeroReducer<
		SumReducer<
			Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>,
			Zeroable<Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>>,
		>,
	>;
	type ReduceC = NonzeroReducer<
		SumReducer<
			Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>,
			Zeroable<Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>>,
		>,
	>;

	fn reducers(self) -> (I, Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		(
			self.i,
			MostDistinctReduceFactory(
				self.n,
				self.probability,
				self.tolerance,
				self.error_rate,
				PhantomData,
			),
			NonzeroReduceFactory(SumReduceFactory::new()),
			NonzeroReducer(SumReducer::new(Zeroable::Nonzero(Top::new(
				self.n,
				self.probability,
				self.tolerance,
				self.error_rate,
			)))),
		)
	}
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct MostDistinctReduceFactory<A, B>(usize, f64, f64, f64, PhantomData<fn(A, B)>);
impl<A, B> Factory for MostDistinctReduceFactory<A, B>
where
	A: Clone + Hash + Eq,
	B: Hash,
{
	type Item = MostDistinctReducer<A, B>;
	fn make(&self) -> Self::Item {
		MostDistinctReducer(Some(Top::new(self.0, self.1, self.2, self.3)))
	}
}
impl<A, B> Clone for MostDistinctReduceFactory<A, B> {
	fn clone(&self) -> Self {
		Self(self.0, self.1, self.2, self.3, PhantomData)
	}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
#[serde(bound(
	serialize = "A: Hash + Eq + Serialize",
	deserialize = "A: Hash + Eq + Deserialize<'de>"
))]
pub struct MostDistinctReducer<A, B: Hash>(
	Option<Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>>,
);

impl<A, B> Reducer for MostDistinctReducer<A, B>
where
	A: Clone + Hash + Eq,
	B: Hash,
{
	type Item = (A, B);
	type Output = Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>;
	type Async = Self;

	fn into_async(self) -> Self::Async {
		self
	}
}
impl<A, B> ReducerAsync for MostDistinctReducer<A, B>
where
	A: Clone + Hash + Eq,
	B: Hash,
{
	type Item = (A, B);
	type Output = Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>;

	#[inline(always)]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context,
		mut stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let self_ = self.project();
		let self_0 = self_.0.as_mut().unwrap();
		while let Some(item) = ready!(stream.as_mut().poll_next(cx)) {
			self_0.push(item.0, &item.1);
		}
		Poll::Ready(())
	}
	fn poll_output(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
		Poll::Ready(self.project().0.take().unwrap())
	}
}
impl<A, B> ReducerProcessSend for MostDistinctReducer<A, B>
where
	A: Clone + Hash + Eq + ProcessSend,
	B: Hash + 'static,
{
	type Output = Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>;
}
impl<A, B> ReducerSend for MostDistinctReducer<A, B>
where
	A: Clone + Hash + Eq + Send + 'static,
	B: Hash + 'static,
{
	type Output = Top<A, streaming_algorithms::HyperLogLogMagnitude<B>>;
}